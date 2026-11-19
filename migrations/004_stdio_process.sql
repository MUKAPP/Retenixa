ALTER TABLE workflows ADD COLUMN stdio_process_json TEXT
    CHECK (stdio_process_json IS NULL OR binding_hash IS NULL);
