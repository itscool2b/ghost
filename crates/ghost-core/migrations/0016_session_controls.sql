-- Independent from legacy autonomy and flat option configuration.
ALTER TABLE agent_sessions ADD COLUMN controls_json TEXT;
