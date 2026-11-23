-- seed: global_settings
INSERT INTO global_settings (id, option_name, option_value) VALUES (1, 'server_api', '50');
INSERT INTO global_settings (id, option_name, option_value) VALUES (2, 'currentProfile', '1');
INSERT INTO global_settings (id, option_name, option_value) VALUES (3, 'currentFormat', '1');
INSERT INTO global_settings (id, option_name, option_value) VALUES (4, 'currentHeat', NULL);
INSERT INTO global_settings (id, option_name, option_value) VALUES (5, 'MinLapSec', '10');
INSERT INTO global_settings (id, option_name, option_value) VALUES (6, 'MinFirstCrossingSec', '0');
INSERT INTO global_settings (id, option_name, option_value) VALUES (7, 'eventName', '2026-11-22 FPV Race');
INSERT INTO global_settings (id, option_name, option_value) VALUES (8, 'eventDescription', '');
INSERT INTO global_settings (id, option_name, option_value) VALUES (9, 'eventResults_cacheStatus', NULL);

-- seed: profiles
INSERT INTO profiles (id, name, description, frequencies, enter_ats, exit_ats, f_ratio) VALUES (1, 'Default', NULL, '{"b": ["R", "R", "F", "F", "R", "R", null, null], "c": [1, 2, 2, 4, 7, 8, null, null], "f": [5658, 5695, 5760, 5800, 5880, 5917, 0, 0]}', '{"v": [90, 90, 90, 90, 90, 90, 90, 90]}', '{"v": [80, 80, 80, 80, 80, 80, 80, 80]}', NULL);

-- seed: race_format
INSERT INTO race_format (id, name, race_mode, race_time_sec, lap_grace_sec, staging_fixed_tones, start_delay_min_ms, start_delay_max_ms, staging_tones, number_laps_win, win_condition, team_racing_mode, start_behavior, points_method) VALUES (1, '2:00 Standard Race', 0, 120, 60, 3, 500, 3500, 0, 0, 1, 0, 0, NULL);
INSERT INTO race_format (id, name, race_mode, race_time_sec, lap_grace_sec, staging_fixed_tones, start_delay_min_ms, start_delay_max_ms, staging_tones, number_laps_win, win_condition, team_racing_mode, start_behavior, points_method) VALUES (2, '1:30 Whoop Sprint', 0, 90, 60, 3, 500, 3500, 0, 0, 1, 0, 0, NULL);
INSERT INTO race_format (id, name, race_mode, race_time_sec, lap_grace_sec, staging_fixed_tones, start_delay_min_ms, start_delay_max_ms, staging_tones, number_laps_win, win_condition, team_racing_mode, start_behavior, points_method) VALUES (3, '3:00 Extended Race', 0, 180, 60, 3, 500, 3500, 0, 0, 1, 0, 0, NULL);
INSERT INTO race_format (id, name, race_mode, race_time_sec, lap_grace_sec, staging_fixed_tones, start_delay_min_ms, start_delay_max_ms, staging_tones, number_laps_win, win_condition, team_racing_mode, start_behavior, points_method) VALUES (4, 'First to 3 Laps', 1, 0, -1, 3, 500, 3500, 0, 3, 2, 0, 0, NULL);
INSERT INTO race_format (id, name, race_mode, race_time_sec, lap_grace_sec, staging_fixed_tones, start_delay_min_ms, start_delay_max_ms, staging_tones, number_laps_win, win_condition, team_racing_mode, start_behavior, points_method) VALUES (5, 'Open Practice', 1, 0, -1, 3, 500, 3500, 0, 0, 0, 0, 0, NULL);
INSERT INTO race_format (id, name, race_mode, race_time_sec, lap_grace_sec, staging_fixed_tones, start_delay_min_ms, start_delay_max_ms, staging_tones, number_laps_win, win_condition, team_racing_mode, start_behavior, points_method) VALUES (6, 'Fastest Lap Qualifier', 0, 120, 60, 3, 500, 3500, 0, 0, 3, 0, 0, NULL);
INSERT INTO race_format (id, name, race_mode, race_time_sec, lap_grace_sec, staging_fixed_tones, start_delay_min_ms, start_delay_max_ms, staging_tones, number_laps_win, win_condition, team_racing_mode, start_behavior, points_method) VALUES (7, 'Fastest Consecutive Laps Qualifier', 0, 120, 60, 3, 500, 3500, 0, 0, 4, 0, 0, NULL);
INSERT INTO race_format (id, name, race_mode, race_time_sec, lap_grace_sec, staging_fixed_tones, start_delay_min_ms, start_delay_max_ms, staging_tones, number_laps_win, win_condition, team_racing_mode, start_behavior, points_method) VALUES (8, 'Lap Count Only', 0, 120, -1, 3, 500, 3500, 0, 0, 5, 0, 0, NULL);
INSERT INTO race_format (id, name, race_mode, race_time_sec, lap_grace_sec, staging_fixed_tones, start_delay_min_ms, start_delay_max_ms, staging_tones, number_laps_win, win_condition, team_racing_mode, start_behavior, points_method) VALUES (9, 'Team / Most Laps Wins', 0, 120, 60, 3, 500, 3500, 0, 0, 1, 1, 0, NULL);
INSERT INTO race_format (id, name, race_mode, race_time_sec, lap_grace_sec, staging_fixed_tones, start_delay_min_ms, start_delay_max_ms, staging_tones, number_laps_win, win_condition, team_racing_mode, start_behavior, points_method) VALUES (10, 'Team / First to 7 Laps', 1, 0, -1, 3, 500, 3500, 0, 7, 2, 1, 0, NULL);
INSERT INTO race_format (id, name, race_mode, race_time_sec, lap_grace_sec, staging_fixed_tones, start_delay_min_ms, start_delay_max_ms, staging_tones, number_laps_win, win_condition, team_racing_mode, start_behavior, points_method) VALUES (11, 'Team / Fastest Lap Average', 0, 120, 60, 3, 500, 3500, 0, 0, 3, 1, 0, NULL);
INSERT INTO race_format (id, name, race_mode, race_time_sec, lap_grace_sec, staging_fixed_tones, start_delay_min_ms, start_delay_max_ms, staging_tones, number_laps_win, win_condition, team_racing_mode, start_behavior, points_method) VALUES (12, 'Team / Fastest Consecutive Average', 0, 120, 60, 3, 500, 3500, 0, 0, 4, 1, 0, NULL);
INSERT INTO race_format (id, name, race_mode, race_time_sec, lap_grace_sec, staging_fixed_tones, start_delay_min_ms, start_delay_max_ms, staging_tones, number_laps_win, win_condition, team_racing_mode, start_behavior, points_method) VALUES (13, 'Co-op / Fastest Time to 7 Laps', 1, 0, -1, 3, 500, 3500, 0, 7, 2, 2, 0, NULL);
INSERT INTO race_format (id, name, race_mode, race_time_sec, lap_grace_sec, staging_fixed_tones, start_delay_min_ms, start_delay_max_ms, staging_tones, number_laps_win, win_condition, team_racing_mode, start_behavior, points_method) VALUES (14, 'Co-op / Most Laps in 2:30', 0, 150, 60, 3, 500, 3500, 0, 0, 1, 2, 0, NULL);

