DROP TRIGGER search_insert;
DROP TRIGGER search_delete;
DROP TRIGGER search_update;
DROP TABLE search_fts;

ALTER TABLE search_documents ADD COLUMN key_tokens TEXT NOT NULL DEFAULT '';
ALTER TABLE search_documents ADD COLUMN condition_tokens TEXT NOT NULL DEFAULT '';
ALTER TABLE search_documents ADD COLUMN tag_tokens TEXT NOT NULL DEFAULT '';
CREATE VIRTUAL TABLE search_fts USING fts5(tokenized,key_tokens,condition_tokens,tag_tokens,content='search_documents',content_rowid='row_id',tokenize='unicode61');
CREATE TRIGGER search_insert AFTER INSERT ON search_documents BEGIN
    INSERT INTO search_fts(rowid,tokenized,key_tokens,condition_tokens,tag_tokens)
    VALUES(new.row_id,new.tokenized,new.key_tokens,new.condition_tokens,new.tag_tokens);
END;
CREATE TRIGGER search_delete AFTER DELETE ON search_documents BEGIN
    INSERT INTO search_fts(search_fts,rowid,tokenized,key_tokens,condition_tokens,tag_tokens)
    VALUES('delete',old.row_id,old.tokenized,old.key_tokens,old.condition_tokens,old.tag_tokens);
END;
CREATE TRIGGER search_update AFTER UPDATE OF row_id,tokenized,key_tokens,condition_tokens,tag_tokens ON search_documents BEGIN
    INSERT INTO search_fts(search_fts,rowid,tokenized,key_tokens,condition_tokens,tag_tokens)
    VALUES('delete',old.row_id,old.tokenized,old.key_tokens,old.condition_tokens,old.tag_tokens);
    INSERT INTO search_fts(rowid,tokenized,key_tokens,condition_tokens,tag_tokens)
    VALUES(new.row_id,new.tokenized,new.key_tokens,new.condition_tokens,new.tag_tokens);
END;

-- Populate a coherent transitional index for offline maintenance. Store::open
-- regenerates all field tokens from authoritative revisions and checkpoints.
INSERT INTO search_fts(search_fts) VALUES('rebuild');
DELETE FROM search_meta WHERE key='tokenizer_version';
