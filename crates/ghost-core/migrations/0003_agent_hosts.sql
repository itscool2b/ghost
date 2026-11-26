-- Older chats were native. Keep the host separate from the provider and transport identifiers.
-- Unknown future host JSON is preserved and refused at launch by older clients.
ALTER TABLE chats ADD COLUMN agent_host TEXT NOT NULL DEFAULT '{"type":"native"}';
UPDATE agent_sessions SET host = '{"type":"native"}' WHERE host = 'native';
