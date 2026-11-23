CREATE TABLE deleted_race (
	id INTEGER NOT NULL, 
	race_id INTEGER NOT NULL, 
	round_id INTEGER NOT NULL, 
	heat_id INTEGER NOT NULL, 
	class_id INTEGER, 
	format_id INTEGER, 
	start_time_formatted VARCHAR, 
	deleted_time_formatted VARCHAR NOT NULL, 
	data VARCHAR NOT NULL, 
	PRIMARY KEY (id)
);

CREATE TABLE global_settings (
	id INTEGER NOT NULL, 
	option_name VARCHAR(40) NOT NULL, 
	option_value VARCHAR, 
	PRIMARY KEY (id)
);

CREATE TABLE heat (
	id INTEGER NOT NULL, 
	note VARCHAR(80), 
	auto_name VARCHAR(80), 
	class_id INTEGER, 
	results BLOB, 
	"cacheStatus" VARCHAR(16) NOT NULL, 
	"order" INTEGER, 
	status INTEGER NOT NULL, 
	auto_frequency BOOLEAN NOT NULL, 
	group_id INTEGER NOT NULL, 
	active BOOLEAN NOT NULL, 
	coop_best_time FLOAT, 
	coop_num_laps INTEGER, 
	PRIMARY KEY (id), 
	FOREIGN KEY(class_id) REFERENCES race_class (id)
);

CREATE TABLE heat_attribute (
	id INTEGER NOT NULL, 
	name VARCHAR(80) NOT NULL, 
	value VARCHAR, 
	PRIMARY KEY (id, name), 
	UNIQUE (id, name), 
	FOREIGN KEY(id) REFERENCES heat (id)
);

CREATE TABLE heat_node (
	id INTEGER NOT NULL, 
	heat_id INTEGER NOT NULL, 
	node_index INTEGER, 
	pilot_id INTEGER, 
	color VARCHAR(6), 
	method INTEGER NOT NULL, 
	seed_rank INTEGER, 
	seed_id INTEGER, 
	PRIMARY KEY (id), 
	UNIQUE (heat_id, node_index), 
	FOREIGN KEY(heat_id) REFERENCES heat (id), 
	FOREIGN KEY(pilot_id) REFERENCES pilot (id)
);

CREATE TABLE lap_split (
	id INTEGER NOT NULL, 
	node_index INTEGER NOT NULL, 
	pilot_id INTEGER, 
	lap_id INTEGER NOT NULL, 
	split_id INTEGER NOT NULL, 
	split_time_stamp INTEGER NOT NULL, 
	split_time INTEGER NOT NULL, 
	split_time_formatted INTEGER NOT NULL, 
	split_speed FLOAT, 
	PRIMARY KEY (id), 
	UNIQUE (node_index, lap_id, split_id), 
	FOREIGN KEY(pilot_id) REFERENCES pilot (id)
);

CREATE TABLE pilot (
	id INTEGER NOT NULL, 
	callsign VARCHAR(80) NOT NULL, 
	team VARCHAR(80) NOT NULL, 
	phonetic VARCHAR(80) NOT NULL, 
	name VARCHAR(120) NOT NULL, 
	color VARCHAR(7), 
	used_frequencies VARCHAR, 
	active BOOLEAN NOT NULL, 
	PRIMARY KEY (id)
);

CREATE TABLE pilot_attribute (
	id INTEGER NOT NULL, 
	name VARCHAR(80) NOT NULL, 
	value VARCHAR, 
	PRIMARY KEY (id, name), 
	UNIQUE (id, name), 
	FOREIGN KEY(id) REFERENCES pilot (id)
);

CREATE TABLE profiles (
	id INTEGER NOT NULL, 
	name VARCHAR(80) NOT NULL, 
	description VARCHAR(256), 
	frequencies VARCHAR(80) NOT NULL, 
	enter_ats VARCHAR(80), 
	exit_ats VARCHAR(80), 
	f_ratio INTEGER, 
	PRIMARY KEY (id)
);

CREATE TABLE race_class (
	id INTEGER NOT NULL, 
	name VARCHAR(80), 
	description VARCHAR(256), 
	format_id INTEGER, 
	win_condition VARCHAR NOT NULL, 
	results BLOB, 
	"cacheStatus" VARCHAR(16) NOT NULL, 
	ranking BLOB, 
	rank_settings VARCHAR, 
	"rankStatus" VARCHAR(16) NOT NULL, 
	rounds INTEGER NOT NULL, 
	"heatAdvanceType" INTEGER NOT NULL, 
	"roundType" INTEGER NOT NULL, 
	"order" INTEGER, 
	active BOOLEAN NOT NULL, 
	PRIMARY KEY (id), 
	FOREIGN KEY(format_id) REFERENCES race_format (id)
);

CREATE TABLE race_class_attribute (
	id INTEGER NOT NULL, 
	name VARCHAR(80) NOT NULL, 
	value VARCHAR, 
	PRIMARY KEY (id, name), 
	UNIQUE (id, name), 
	FOREIGN KEY(id) REFERENCES race_class (id)
);

CREATE TABLE race_format (
	id INTEGER NOT NULL, 
	name VARCHAR(80) NOT NULL, 
	race_mode INTEGER NOT NULL, 
	race_time_sec INTEGER NOT NULL, 
	lap_grace_sec INTEGER NOT NULL, 
	staging_fixed_tones INTEGER NOT NULL, 
	start_delay_min_ms INTEGER NOT NULL, 
	start_delay_max_ms INTEGER NOT NULL, 
	staging_tones INTEGER NOT NULL, 
	number_laps_win INTEGER NOT NULL, 
	win_condition INTEGER NOT NULL, 
	team_racing_mode INTEGER NOT NULL, 
	start_behavior INTEGER NOT NULL, 
	points_method VARCHAR, 
	PRIMARY KEY (id)
);

CREATE TABLE race_format_attribute (
	id INTEGER NOT NULL, 
	name VARCHAR(80) NOT NULL, 
	value VARCHAR, 
	PRIMARY KEY (id, name), 
	UNIQUE (id, name), 
	FOREIGN KEY(id) REFERENCES race_format (id)
);

CREATE TABLE saved_pilot_race (
	id INTEGER NOT NULL, 
	race_id INTEGER NOT NULL, 
	node_index INTEGER NOT NULL, 
	pilot_id INTEGER, 
	history_values VARCHAR, 
	history_times VARCHAR, 
	penalty_time INTEGER NOT NULL, 
	penalty_desc VARCHAR, 
	enter_at INTEGER NOT NULL, 
	exit_at INTEGER NOT NULL, 
	frequency INTEGER, 
	marshal_type INTEGER, 
	PRIMARY KEY (id), 
	UNIQUE (race_id, node_index), 
	FOREIGN KEY(race_id) REFERENCES saved_race_meta (id), 
	FOREIGN KEY(pilot_id) REFERENCES pilot (id)
);

CREATE TABLE saved_race_lap (
	id INTEGER NOT NULL, 
	race_id INTEGER NOT NULL, 
	pilotrace_id INTEGER NOT NULL, 
	node_index INTEGER NOT NULL, 
	pilot_id INTEGER, 
	lap_time_stamp FLOAT NOT NULL, 
	lap_time FLOAT NOT NULL, 
	lap_time_formatted VARCHAR NOT NULL, 
	source INTEGER NOT NULL, 
	deleted BOOLEAN NOT NULL, 
	peak_rssi INTEGER, 
	PRIMARY KEY (id), 
	FOREIGN KEY(race_id) REFERENCES saved_race_meta (id), 
	FOREIGN KEY(pilotrace_id) REFERENCES saved_pilot_race (id), 
	FOREIGN KEY(pilot_id) REFERENCES pilot (id)
);

CREATE TABLE saved_race_lap_split (
	id INTEGER NOT NULL, 
	race_id INTEGER NOT NULL, 
	pilotrace_id INTEGER NOT NULL, 
	node_index INTEGER NOT NULL, 
	pilot_id INTEGER, 
	lap_id INTEGER NOT NULL, 
	split_id INTEGER NOT NULL, 
	split_time_stamp FLOAT NOT NULL, 
	split_time FLOAT NOT NULL, 
	split_time_formatted VARCHAR, 
	split_speed FLOAT, 
	speed_only BOOLEAN NOT NULL, 
	PRIMARY KEY (id), 
	FOREIGN KEY(race_id) REFERENCES saved_race_meta (id), 
	FOREIGN KEY(pilotrace_id) REFERENCES saved_pilot_race (id), 
	FOREIGN KEY(pilot_id) REFERENCES pilot (id)
);

CREATE TABLE saved_race_meta (
	id INTEGER NOT NULL, 
	round_id INTEGER NOT NULL, 
	heat_id INTEGER NOT NULL, 
	class_id INTEGER, 
	format_id INTEGER, 
	start_time FLOAT NOT NULL, 
	start_time_formatted VARCHAR NOT NULL, 
	results BLOB, 
	"cacheStatus" VARCHAR(16) NOT NULL, 
	PRIMARY KEY (id), 
	UNIQUE (round_id, heat_id), 
	FOREIGN KEY(heat_id) REFERENCES heat (id), 
	FOREIGN KEY(class_id) REFERENCES race_class (id), 
	FOREIGN KEY(format_id) REFERENCES race_format (id)
);

CREATE TABLE saved_race_meta_attribute (
	id INTEGER NOT NULL, 
	name VARCHAR(80) NOT NULL, 
	value VARCHAR, 
	PRIMARY KEY (id, name), 
	UNIQUE (id, name), 
	FOREIGN KEY(id) REFERENCES saved_race_meta (id)
);

