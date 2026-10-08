ALTER TABLE runs ADD COLUMN title TEXT COLLATE "C";
WITH whitespace(chars) AS (
    VALUES (U&'\0009\000b\000c\000d\0020\0085\00a0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200a\2028\2029\202f\205f\3000')
), message_lines AS (
    SELECT r.id, btrim(split_part(COALESCE(m.text_content, ''), E'\n', 1), (SELECT chars FROM whitespace)) AS line
    FROM runs r LEFT JOIN messages m ON m.id = r.trigger_ref AND m.workspace_id = r.workspace_id
), message_titles AS (
    SELECT id, CASE
        WHEN line = '' THEN 'A message with an attachment'
        WHEN char_length(line) <= 80 THEN line
        ELSE rtrim(left(line, COALESCE((
            SELECT max(n) - 1 FROM generate_series(1, 79) AS n
            WHERE strpos((SELECT chars FROM whitespace), substr(line, n, 1)) > 0
        ), 79)), (SELECT chars FROM whitespace)) || '…'
    END AS title FROM message_lines
)
UPDATE runs r SET title = CASE r.trigger_kind
    WHEN 'message' THEN mt.title
    WHEN 'arrival' THEN 'Bring a source into memory'
    WHEN 'review' THEN 'Review what was learned'
    ELSE COALESCE((SELECT rule_name FROM wakeups w WHERE w.id = r.trigger_ref AND w.workspace_id = r.workspace_id),
        (SELECT 'Call from ' || remote_e164 FROM calls c WHERE c.run_id = r.id AND c.direction = 'inbound'),
        'An event') END
FROM message_titles mt WHERE mt.id = r.id;
ALTER TABLE runs ALTER COLUMN title SET NOT NULL;
