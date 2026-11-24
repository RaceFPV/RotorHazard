//! Heat generators — port of HeatGenerator.py (the generation/apply
//! framework), rh_heatgenerator_standard (FAI bracket tables, extracted
//! verbatim from the Python plugin into data/heat_brackets.json via
//! data/extract_heat_brackets.py) and rh_heatgenerator_ladder (ladder +
//! balanced random fill).

use serde_json::{json, Value};
use std::collections::HashMap;

// SeedMethod (HeatGenerator.py)
pub const SEED_INPUT: i64 = 0;
pub const SEED_HEAT_INDEX: i64 = 1;
pub const SEED_CLASS_INDEX: i64 = 2;

// ProgramMethod (Database.py)
pub const PROGRAM_NONE: i64 = -1;
pub const PROGRAM_ASSIGN: i64 = 0;
pub const PROGRAM_HEAT_RESULT: i64 = 1;
pub const PROGRAM_CLASS_RESULT: i64 = 2;

const BRACKETS_JSON: &str = include_str!("data/heat_brackets.json");

/// One planned slot: how the seat is seeded.
#[derive(Clone, Debug)]
pub struct PlanSlot {
    pub method: i64,
    pub seed_rank: i64,
    pub seed_index: Option<i64>,
}

/// One planned heat.
#[derive(Clone, Debug)]
pub struct PlanHeat {
    pub name: String,
    pub slots: Vec<PlanSlot>,
}

/// Loads the FAI bracket plans for a standard spec name
/// ("fai16"/"fai32"/"fai64"/"multigp16" x 1e/2e).
fn load_bracket(spec: &str) -> Option<Vec<PlanHeat>> {
    let table: Value = serde_json::from_str(BRACKETS_JSON).ok()?;
    let entry = match spec {
        "fai16" => &table["bracket_1e_16_fai"],
        "fai32" => &table["bracket_1e_32_fai"],
        "fai64" => &table["bracket_1e_64_fai"],
        "2e_fai16" => &table["bracket_2e_16_fai"],
        "2e_fai32" => &table["bracket_2e_32_fai"],
        "2e_fai64" => &table["bracket_2e_64_fai"],
        "2e_multigp16" => &table["bracket_2e_16_multigp"],
        _ => return None,
    };
    let plans = entry.as_array()?;
    Some(
        plans
            .iter()
            .map(|p| PlanHeat {
                name: p.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                slots: p
                    .get("slots")
                    .and_then(|v| v.as_array())
                    .map(|slots| {
                        slots
                            .iter()
                            .map(|s| PlanSlot {
                                method: s.get("method").and_then(|v| v.as_i64()).unwrap_or(SEED_INPUT),
                                seed_rank: s.get("seed_rank").and_then(|v| v.as_i64()).unwrap_or(0),
                                seed_index: s.get("seed_index").and_then(|v| v.as_i64()),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .collect(),
    )
}

fn apply_seed_offset(plans: &mut [PlanHeat], offset: i64) {
    if offset > 0 {
        for heat in plans {
            for slot in &mut heat.slots {
                if slot.method == SEED_INPUT {
                    slot.seed_rank += offset;
                }
            }
        }
    }
}

/// Deterministic shuffle (same LCG as the mock providers).
fn seeded_shuffle(items: &mut [i64], seed: u64) {
    let mut seed = seed | 1;
    for i in (1..items.len()).rev() {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let j = ((seed >> 33) as usize) % (i + 1);
        items.swap(i, j);
    }
}

/// Alphabet generator: A..Z, AA.. (letter_generator in Python).
fn heat_letter(index: usize) -> String {
    let mut n = index as i64;
    let mut out = Vec::new();
    loop {
        out.push((b'A' + (n % 26) as u8) as char);
        n = n / 26 - 1;
        if n < 0 {
            break;
        }
    }
    out.reverse();
    out.into_iter().collect()
}



fn heats_by_class(conn: &rusqlite::Connection, class_id: i64) -> Vec<(i64, String)> {
    let mut stmt = conn
        .prepare("SELECT id FROM heat WHERE class_id = ?1 ORDER BY id")
        .unwrap();
    let mut r = stmt.query([class_id]).unwrap();
    let mut out = Vec::new();
    while let Some(row) = r.next().unwrap() {
        out.push(row.get::<_, i64>(0).unwrap());
    }
    out.into_iter()
        .map(|id| {
            let name: String = conn
                .query_row(
                    "SELECT COALESCE(NULLIF(note, ''), COALESCE(NULLIF(auto_name, ''), 'Heat ' || id)) FROM heat WHERE id = ?1",
                    [id],
                    |r| r.get(0),
                )
                .unwrap_or_default();
            (id, name)
        })
        .collect()
}

/// Ladder generator (generateLadder): heats are built back-to-front —
/// the last heat qualifies everyone remaining; earlier heats advance a
/// fixed number into the heat that follows them.
fn generate_ladder(
    available_seats: usize,
    qualifiers_per_heat: Option<i64>,
    advances_per_heat: Option<i64>,
    total_pilots: usize,
    seed_offset: i64,
    suffix: &str,
) -> Option<Vec<PlanHeat>> {
    let (qualifiers, advances) = match (qualifiers_per_heat, advances_per_heat) {
        (Some(q), Some(a)) => (q, a),
        (None, Some(a)) => (available_seats as i64 - a, a),
        (Some(q), None) => (q, available_seats as i64 - q),
        (None, None) => (available_seats as i64 - 1, 1),
    };
    if (qualifiers < 1 || advances < 1) && !(advances_per_heat == Some(0)) {
        tracing::warn!("Unable to seed ladder: provided qualifiers and advances must be > 0");
        return None;
    }
    if total_pilots == 0 {
        tracing::warn!("Unable to seed ladder: no pilots available");
        return None;
    }

    // built back-to-front, exactly as Python: a heat in progress stays
    // at the end of the list and is prepended only when it completes
    // (fills with qualifiers, or takes the remainder/advances)
    let mut heats: Vec<PlanHeat> = Vec::new();
    let mut current: Option<PlanHeat> = None;
    let mut unseeded: Vec<i64> = (seed_offset..(total_pilots as i64 + seed_offset)).collect();
    let mut heat_pilots = 0usize;

    while !unseeded.is_empty() {
        if heat_pilots == 0 {
            let letter = heat_letter(heats.len());
            current = Some(PlanHeat {
                name: format!("{} {}", letter, suffix),
                slots: Vec::new(),
            });
        }

        let heat = current.as_mut().expect("heat in progress");
        if (heat_pilots as i64) < qualifiers {
            let seed = unseeded.remove(0);
            heat.slots.push(PlanSlot {
                method: SEED_INPUT,
                seed_rank: seed + 1,
                seed_index: None,
            });
            heat_pilots += 1;
        } else {
            if unseeded.len() <= advances as usize {
                // slot remainder as qualifiers
                for seed in unseeded.drain(..) {
                    heat.slots.push(PlanSlot {
                        method: SEED_INPUT,
                        seed_rank: seed + 1,
                        seed_index: None,
                    });
                }
            } else {
                // slot advances: HEAT_INDEX seeds counted negatively from
                // the current list length; apply() resolves them against
                // the plan-order mapping the same way Python does
                for adv_idx in 0..advances {
                    heat.slots.push(PlanSlot {
                        method: SEED_HEAT_INDEX,
                        seed_rank: adv_idx + 1,
                        seed_index: Some(-(heats.len() as i64) - 2),
                    });
                }
            }

            // commit the completed heat to the front
            heats.insert(0, current.take().expect("heat in progress"));
            heat_pilots = 0;
        }
    }

    if heat_pilots > 0 {
        // insert the final (incomplete) heat
        if let Some(heat) = current.take() {
            heats.insert(0, heat);
        }
    }

    Some(heats)
}

/// Balanced random fill (generateBalancedHeats): heats sized to the seat
/// count, pilots shuffled across them in round-robin order.
fn generate_balanced(
    available_seats: usize,
    qualifiers_per_heat: Option<i64>,
    total_pilots: usize,
    seed_offset: i64,
    suffix: &str,
    seed: u64,
) -> Option<Vec<PlanHeat>> {
    let qualifiers = qualifiers_per_heat.unwrap_or(available_seats as i64);
    if qualifiers < 1 {
        tracing::warn!("Unable to seed heats: provided qualifiers must be > 1");
        return None;
    }
    if total_pilots == 0 {
        tracing::warn!("Unable to seed heats: no pilots available");
        return None;
    }
    let mut total_heats = total_pilots as i64 / qualifiers;
    if total_pilots as i64 % qualifiers != 0 {
        total_heats += 1;
    }

    let mut heats: Vec<PlanHeat> = (0..total_heats)
        .map(|idx| PlanHeat {
            name: format!("{} {}", heat_letter(idx as usize), suffix),
            slots: Vec::new(),
        })
        .collect();

    let mut unseeded: Vec<i64> = (seed_offset..(total_pilots as i64 + seed_offset)).collect();
    seeded_shuffle(&mut unseeded, seed);

    let mut heat_num = 0usize;
    for seed in unseeded {
        if heat_num >= heats.len() {
            heat_num = 0;
        }
        heats[heat_num].slots.push(PlanSlot {
            method: SEED_INPUT,
            seed_rank: seed + 1,
            seed_index: None,
        });
        heat_num += 1;
    }

    Some(heats)
}

/// Generates the heat plans for the named generator (the generator
/// functions in the Python plugins).
pub fn generate_plans(
    conn: &rusqlite::Connection,
    generator_name: &str,
    args: &Value,
    available_seats: usize,
) -> Option<Vec<PlanHeat>> {
    let get = |k: &str| args.get(k).and_then(|v| v.as_i64());
    let seed_offset = get("seed_offset").map(|v| (v - 1).max(0)).unwrap_or(0);
    let input_class = args.get("input_class").and_then(|v| v.as_i64());
    let total = total_pilots_resolved(conn, input_class, get("total_pilots"));

    match generator_name {
        n if n == crate::export_import::clean_var_name("Regulation bracket, single elimination") => {
            let spec = args.get("standard").and_then(|v| v.as_str())?;
            let mut plans = load_bracket(spec)?;
            apply_seed_offset(&mut plans, seed_offset);
            Some(plans)
        }
        n if n == crate::export_import::clean_var_name("Regulation bracket, double elimination") => {
            let spec = args.get("standard").and_then(|v| v.as_str())?;
            let mut plans = load_bracket(spec)?;
            apply_seed_offset(&mut plans, seed_offset);
            Some(plans)
        }
        n if n == crate::export_import::clean_var_name("Ranked fill") => generate_ladder(
            available_seats,
            get("qualifiers_per_heat"),
            get("advances_per_heat"),
            total,
            get("seed_offset").unwrap_or(0),
            args.get("suffix").and_then(|v| v.as_str()).unwrap_or("Main"),
        ),
        n if n == crate::export_import::clean_var_name("Balanced random fill") => generate_balanced(
            available_seats,
            get("qualifiers_per_heat"),
            total,
            seed_offset,
            args.get("suffix").and_then(|v| v.as_str()).unwrap_or("Qualifier"),
            crate::state::now_epoch_ms() as u64,
        ),
        _ => None,
    }
}

/// Resolves a generator's label from its clean-var-name id.
pub fn generator_label(generator_id: &str) -> Option<&'static str> {
    const LABELS: &[&str] = &[
        "Regulation bracket, single elimination",
        "Regulation bracket, double elimination",
        "Ranked fill",
        "Balanced random fill",
        "Ladder",
    ];
    LABELS
        .iter()
        .find(|l| crate::export_import::clean_var_name(l) == generator_id)
        .copied()
}

/// The registered generators for the 'heatgenerator_list' load type.
pub fn generator_list() -> Value {
    let gens = json!([
        {
            "name": crate::export_import::clean_var_name("Regulation bracket, single elimination"),
            "label": "Regulation bracket, single elimination",
            "settings": [
                {"name": "standard", "label": "Spec", "field_type": "SELECT",
                 "options": [
                     {"value": "fai16", "label": "FAI, 4-up, 16-pilot"},
                     {"value": "fai32", "label": "FAI, 4-up, 32-pilot"},
                     {"value": "fai64", "label": "FAI, 4-up, 64-pilot"},
                 ]},
                {"name": "seed_offset", "label": "Seed from rank", "field_type": "BASIC_INT", "value": 1},
            ],
        },
        {
            "name": crate::export_import::clean_var_name("Regulation bracket, double elimination"),
            "label": "Regulation bracket, double elimination",
            "settings": [
                {"name": "standard", "label": "Spec", "field_type": "SELECT",
                 "options": [
                     {"value": "fai16", "label": "FAI, 4-up, 16-pilot"},
                     {"value": "fai32", "label": "FAI, 4-up, 32-pilot"},
                     {"value": "fai64", "label": "FAI, 4-up, 64-pilot"},
                     {"value": "multigp16", "label": "MultiGP, 4-up, 16-pilot"},
                 ]},
                {"name": "seed_offset", "label": "Seed from rank", "field_type": "BASIC_INT", "value": 1},
            ],
        },
        {
            "name": crate::export_import::clean_var_name("Ranked fill"),
            "label": "Ranked fill",
            "settings": [
                {"name": "qualifiers_per_heat", "label": "Maximum pilots per heat", "field_type": "BASIC_INT", "placeholder": "Auto"},
                {"name": "total_pilots", "label": "Maxiumum pilots in class", "field_type": "BASIC_INT", "placeholder": "Auto"},
                {"name": "seed_offset", "label": "Seed from rank", "field_type": "BASIC_INT", "value": 1},
                {"name": "suffix", "label": "Heat title suffix", "field_type": "TEXT", "placeholder": "Main", "value": "Main"},
            ],
        },
        {
            "name": crate::export_import::clean_var_name("Balanced random fill"),
            "label": "Balanced random fill",
            "settings": [
                {"name": "qualifiers_per_heat", "label": "Maximum pilots per heat", "field_type": "BASIC_INT", "placeholder": "Auto"},
                {"name": "total_pilots", "label": "Maxiumum pilots in class", "field_type": "BASIC_INT", "placeholder": "Auto"},
                {"name": "seed_offset", "label": "Seed from rank", "field_type": "BASIC_INT", "value": 1},
                {"name": "suffix", "label": "Heat title suffix", "field_type": "TEXT", "placeholder": "Qualifier", "value": "Qualifier"},
            ],
        },
        {
            "name": crate::export_import::clean_var_name("Ladder"),
            "label": "Ladder",
            "settings": [
                {"name": "advances_per_heat", "label": "Advances per heat", "field_type": "BASIC_INT", "placeholder": "Auto"},
                {"name": "qualifiers_per_heat", "label": "Seeded slots per heat", "field_type": "BASIC_INT", "placeholder": "Auto"},
                {"name": "total_pilots", "label": "Pilots in class", "field_type": "BASIC_INT", "placeholder": "Auto"},
                {"name": "seed_offset", "label": "Seed from rank", "field_type": "BASIC_INT", "value": 1},
                {"name": "suffix", "label": "Heat title suffix", "field_type": "TEXT", "placeholder": "Main", "value": "Main"},
            ],
        },
    ]);
    json!({"generators": gens})
}

/// The apply() half (HeatGeneratorManager.apply): creates the output
/// class (when needed), one heat per plan, and seeds the slots.  Returns
/// the output class id.
pub fn apply(
    conn: &rusqlite::Connection,
    generator_label: &str,
    plans: &[PlanHeat],
    args: &Value,
) -> Option<i64> {
    let input_class: Option<i64> = args.get("input_class").and_then(|v| v.as_i64());
    let mut output_class: Option<i64> = args.get("output_class").and_then(|v| v.as_i64());

    let tx = conn.unchecked_transaction().ok()?;

    if output_class.is_none() {
        let existing: Vec<String> = {
            let mut stmt = tx.prepare("SELECT name FROM race_class").unwrap();
            let rows = stmt.query_map([], |r| r.get(0)).unwrap();
            rows.filter_map(|r| r.ok()).collect()
        };
        let name = crate::db::unique_name(generator_label, &existing);
        tx.execute(
            "INSERT INTO race_class (name, description, format_id, win_condition, \
                                    results, \"cacheStatus\", ranking, rank_settings, \
                                    \"rankStatus\", rounds, \"heatAdvanceType\", \"roundType\", \"order\", points_method)
             VALUES (?1, NULL, NULL, '', NULL, '{}', NULL, NULL, '{}', 0, 0, 0, NULL, NULL)",
            rusqlite::params![name],
        )
        .ok()?;
        output_class = Some(tx.last_insert_rowid());
    }
    let output_class = output_class.unwrap();

    // create heats and their slots
    let mut heat_id_mapping: Vec<i64> = Vec::new();
    for plan in plans {
        let id = crate::db::next_heat_id(&tx);
        tx.execute(
            "INSERT INTO heat (id, note, auto_name, class_id, results, \"cacheStatus\", \"order\",
                               status, auto_frequency, group_id, active, coop_best_time, coop_num_laps)
             VALUES (?1, ?2, NULL, ?3, NULL, '{}', NULL, 0, 1, 0, 1, NULL, NULL)",
            rusqlite::params![id, plan.name, output_class],
        )
        .ok()?;
        for node_index in 0..8i64 {
            tx.execute(
                "INSERT INTO heat_node (id, heat_id, node_index, pilot_id, color, method, seed_rank, seed_id)
                 VALUES (?1, ?2, ?3, NULL, NULL, 0, NULL, NULL)",
                rusqlite::params![crate::db::next_heat_node_id(&tx), id, node_index],
            )
            .ok()?;
        }
        heat_id_mapping.push(id);
    }

    // seed the slots (apply() slot loop)
    let mut pilot_pool: Vec<i64> = Vec::new();
    let mut filled_pool = false;

    // input-class results pool (ordered), used when the input class's
    // format has no win condition
    let mut input_pool: Vec<i64> = Vec::new();
    if let Some(ic) = input_class {
        let win_condition: Option<i64> = tx
            .query_row(
                "SELECT f.win_condition FROM race_class c \
                 JOIN race_format f ON c.format_id = f.id WHERE c.id = ?1",
                [ic],
                |r| r.get::<_, Option<i64>>(0),
            )
            .ok()
            .flatten();
        if win_condition.is_none() {
            // fill pool from the input class's results, in rank order
            for (heat_id, _) in heats_by_class(&tx, ic) {
                if let Ok(results_json) = tx.query_row(
                    "SELECT results FROM saved_race_meta WHERE heat_id = ?1 AND results IS NOT NULL \
                     ORDER BY id DESC LIMIT 1",
                    [heat_id],
                    |r| r.get::<_, String>(0),
                ) {
                    if let Ok(results) = serde_json::from_str::<Value>(&results_json) {
                        if let Some(board) = results.get("by_race_time").and_then(|v| v.as_array()) {
                            for line in board {
                                if let Some(pid) = line.get("pilot_id").and_then(|v| v.as_i64()) {
                                    input_pool.push(pid);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    for (h_idx, plan) in plans.iter().enumerate() {
        let heat_id = heat_id_mapping[h_idx];
        let slot_ids: Vec<i64> = {
            let mut stmt = tx
                .prepare("SELECT id FROM heat_node WHERE heat_id = ?1 ORDER BY node_index")
                .unwrap();
            let r = stmt.query_map([heat_id], |row| row.get::<_, i64>(0)).unwrap();
            r.filter_map(|row| row.ok()).collect()
        };

        for (s_idx, slot_id) in slot_ids.iter().enumerate() {
            let Some(seed_slot) = plan.slots.get(s_idx) else { break };
            let mut data: HashMap<&str, Value> = HashMap::new();
            data.insert("heat", json!(heat_id));
            data.insert("slot_id", json!(slot_id));
            data.insert("seed_rank", json!(seed_slot.seed_rank));

            match seed_slot.method {
                SEED_INPUT => {
                    if let Some(ic) = input_class {
                        let win_condition: Option<i64> = tx
                            .query_row(
                                "SELECT f.win_condition FROM race_class c \
                                 JOIN race_format f ON c.format_id = f.id WHERE c.id = ?1",
                                [ic],
                                |r| r.get::<_, Option<i64>>(0),
                            )
                            .ok()
                            .flatten();
                        if win_condition.is_some() {
                            data.insert("method", json!(PROGRAM_CLASS_RESULT));
                            data.insert("seed_class_id", json!(ic));
                        } else {
                            if !filled_pool {
                                pilot_pool = input_pool.clone();
                                seeded_shuffle(&mut pilot_pool, crate::state::now_epoch_ms() as u64);
                                filled_pool = true;
                            }
                            pop_into_assign(&mut data, &mut pilot_pool);
                        }
                    } else {
                        if !filled_pool {
                            let mut stmt = tx.prepare("SELECT id FROM pilot ORDER BY id").unwrap();
                            let rows = stmt.query_map([], |r| r.get::<_, i64>(0)).unwrap();
                            pilot_pool = rows.filter_map(|r| r.ok()).collect();
                            seeded_shuffle(&mut pilot_pool, crate::state::now_epoch_ms() as u64);
                            filled_pool = true;
                        }
                        pop_into_assign(&mut data, &mut pilot_pool);
                    }
                }
                SEED_HEAT_INDEX => {
                    data.insert("method", json!(PROGRAM_HEAT_RESULT));
                    // Python indexes the mapping negatively from the end
                    // (seed_index of -1 refers to the heat inserted just
                    // before this one)
                    let idx = heat_id_mapping.len() as i64 + seed_slot.seed_index.unwrap_or(0);
                    if idx >= 0 && (idx as usize) < heat_id_mapping.len() {
                        data.insert("seed_heat_id", json!(heat_id_mapping[idx as usize]));
                    } else {
                        tracing::error!("Heat seed index out of range: {}", seed_slot.seed_index.unwrap_or(0));
                        return None;
                    }
                }
                SEED_CLASS_INDEX => {
                    data.insert("method", json!(PROGRAM_CLASS_RESULT));
                    data.insert("seed_class_id", json!(seed_slot.seed_index.unwrap_or(0)));
                }
                other => {
                    tracing::error!("Not a supported seed method: {}", other);
                    return None;
                }
            }

            // apply the alteration (alter_heatNodes_fast semantics)
            tx.execute(
                "UPDATE heat_node SET method = ?2, seed_rank = ?3 WHERE id = ?1",
                rusqlite::params![slot_id, data.get("method").and_then(|v| v.as_i64()).unwrap_or(PROGRAM_NONE),
                    data.get("seed_rank").and_then(|v| v.as_i64()).unwrap_or(0)],
            )
            .ok()?;
            if let Some(pilot) = data.get("pilot").and_then(|v| v.as_i64()) {
                tx.execute(
                    "UPDATE heat_node SET pilot_id = ?2 WHERE id = ?1",
                    rusqlite::params![slot_id, pilot],
                )
                .ok()?;
            }
            if let Some(seed_heat) = data.get("seed_heat_id").and_then(|v| v.as_i64()) {
                tx.execute(
                    "UPDATE heat_node SET seed_id = ?2 WHERE id = ?1",
                    rusqlite::params![slot_id, seed_heat],
                )
                .ok()?;
            }
            if let Some(seed_class) = data.get("seed_class_id").and_then(|v| v.as_i64()) {
                tx.execute(
                    "UPDATE heat_node SET seed_id = ?2 WHERE id = ?1",
                    rusqlite::params![slot_id, seed_class],
                )
                .ok()?;
            }
        }
    }

    if filled_pool && !pilot_pool.is_empty() {
        tracing::info!("{} unseeded pilots remaining in pool", pilot_pool.len());
    }

    tx.commit().ok()?;
    Some(output_class)
}

fn pop_into_assign(data: &mut HashMap<&str, Value>, pool: &mut Vec<i64>) {
    if pool.is_empty() {
        tracing::info!("Unable to seed pilot: no available pilots left to seed");
        data.insert("method", json!(PROGRAM_NONE));
    } else {
        data.insert("method", json!(PROGRAM_ASSIGN));
        data.insert("pilot", json!(pool.pop().unwrap_or(0)));
    }
}

/// Total pilots resolving through the input class's saved results when
/// available (getTotalPilots).
pub fn total_pilots_resolved(
    conn: &rusqlite::Connection,
    input_class: Option<i64>,
    explicit: Option<i64>,
) -> usize {
    if let Some(t) = explicit {
        return t.max(0) as usize;
    }
    if let Some(class_id) = input_class {
        let count = heats_by_class(conn, class_id)
            .iter()
            .filter_map(|(heat_id, _)| {
                conn.query_row(
                    "SELECT results FROM saved_race_meta WHERE heat_id = ?1 AND results IS NOT NULL \
                     ORDER BY id DESC LIMIT 1",
                    [*heat_id],
                    |r| r.get::<_, String>(0),
                )
                .ok()
            })
            .filter_map(|s| serde_json::from_str::<Value>(&s).ok())
            .map(|r| {
                r.get("by_race_time").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0)
            })
            .sum::<usize>();
        if count > 0 {
            return count;
        }
    }
    conn.query_row("SELECT COUNT(*) FROM pilot", [], |r| r.get::<_, i64>(0))
        .map(|n| n.max(0) as usize)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brackets_embed_and_shape() {
        let plans = load_bracket("fai16").expect("1e_16 present");
        assert_eq!(plans.len(), 8);
        assert_eq!(plans[0].slots.len(), 4);
        // FAI bracket: seed ranks 1/8/9/16 in race 1
        assert_eq!(plans[0].slots[0].seed_rank, 1);
        assert_eq!(plans[0].slots[1].seed_rank, 8);
        assert_eq!(plans[1].slots[0].seed_rank, 4);

        let plans = load_bracket("2e_fai16").expect("2e_16 present");
        assert_eq!(plans.len(), 14); // double elim: 2n-2 heats
        assert_eq!(plans.last().unwrap().name, "Final");

        // the 2e bracket carries HEAT_INDEX seeds (advancing heats)
        assert!(plans
            .iter()
            .any(|p| p.slots.iter().any(|s| s.method == SEED_HEAT_INDEX)));
    }

    #[test]
    fn seed_offset_shifts_input_ranks() {
        let mut plans = load_bracket("fai16").unwrap();
        apply_seed_offset(&mut plans, 3);
        assert_eq!(plans[0].slots[0].seed_rank, 4);
    }

    #[test]
    fn ladder_structure() {
        // 4 seats, 8 pilots: first heat qualifies 3 + advances 1 into it,
        // building back-to-front; final heat has all 8 pilots' seeds
        let plans = generate_ladder(4, None, None, 8, 0, "Main").unwrap();
        // the earliest-inserted heat (last in list order) is the 'A' heat
        // with qualifiers seeded by INPUT; later heats advance into it
        assert!(!plans.is_empty());
        assert!(plans
            .iter()
            .any(|p| p.slots.iter().any(|s| s.method == SEED_HEAT_INDEX)));
        // total input seeds across heats == total pilots
        let total_input: usize = plans
            .iter()
            .map(|p| p.slots.iter().filter(|s| s.method == SEED_INPUT).count())
            .sum();
        assert_eq!(total_input, 8);
        // heat names use letters + suffix
        assert!(plans.iter().any(|p| p.name.ends_with("Main")));
    }

    #[test]
    fn balanced_fill_distributes_all_pilots() {
        let plans = generate_balanced(4, Some(3), 10, 0, "Qualifier", 42).unwrap();
        // 10 pilots / 3 per heat -> 4 heats
        assert_eq!(plans.len(), 4);
        let total: usize = plans.iter().map(|p| p.slots.len()).sum();
        assert_eq!(total, 10);
        // round-robin: no heat over capacity
        assert!(plans.iter().all(|p| p.slots.len() <= 3));
    }

    #[test]
    fn heat_letter_sequences_like_python() {
        assert_eq!(heat_letter(0), "A");
        assert_eq!(heat_letter(25), "Z");
        assert_eq!(heat_letter(26), "AA");
        assert_eq!(heat_letter(27), "AB");
    }
}
