-- 会议材料工作台 初始 schema（v1）
--
-- 设计原则：
-- 1. 文件字节不进数据库，只存路径/哈希/关系（文档第 7 节）
-- 2. documents 是材料实体，meeting_documents / topic_documents 是关系
-- 3. 原文、OCR、AI 结果、用户备注分开存放，永不互相覆盖
-- 4. 所有时间戳为 UTC，格式 YYYY-MM-DD HH:MM:SS

-- ── 会议 ──────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS meetings (
  id             INTEGER PRIMARY KEY AUTOINCREMENT,
  title          TEXT    NOT NULL,
  meeting_date   TEXT,
  location       TEXT,
  status         TEXT    NOT NULL DEFAULT 'active',   -- active | archived
  last_opened_at TEXT,
  created_at     TEXT    NOT NULL DEFAULT (datetime('now','localtime')),
  updated_at     TEXT    NOT NULL DEFAULT (datetime('now','localtime'))
);

-- ── 文件内容实体（内容寻址，全局唯一，字节只存一份）──────────────
CREATE TABLE IF NOT EXISTS blobs (
  sha256     TEXT    PRIMARY KEY,
  rel_path   TEXT    NOT NULL,
  size       INTEGER NOT NULL,
  ref_count  INTEGER NOT NULL DEFAULT 0,
  created_at TEXT    NOT NULL DEFAULT (datetime('now','localtime'))
);

-- ── 材料元数据 ────────────────────────────────────────────────────
-- 同一份内容 + 同一个原始文件名 = 一条记录（避免同内容多文件名互相覆盖）
CREATE TABLE IF NOT EXISTS documents (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  sha256            TEXT    NOT NULL REFERENCES blobs(sha256) ON DELETE RESTRICT,
  filename_original TEXT    NOT NULL,
  filename_display  TEXT    NOT NULL,
  ext               TEXT    NOT NULL,
  mime              TEXT,
  size              INTEGER NOT NULL,
  page_count        INTEGER,
  source_path       TEXT,
  parse_status      TEXT    NOT NULL DEFAULT 'imported',
  -- imported | parsing | parsed | ocr_pending | ocr_running | failed
  parse_error       TEXT,
  text_source       TEXT,   -- text_layer | ocr | parsed | mixed
  created_at        TEXT    NOT NULL DEFAULT (datetime('now','localtime')),
  updated_at        TEXT    NOT NULL DEFAULT (datetime('now','localtime'))
);

CREATE UNIQUE INDEX IF NOT EXISTS uniq_documents_content_name
  ON documents (sha256, filename_original);

CREATE INDEX IF NOT EXISTS idx_documents_status ON documents (parse_status);

-- 同一份内容出现过的其它文件名
CREATE TABLE IF NOT EXISTS document_aliases (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  alias       TEXT    NOT NULL,
  created_at  TEXT    NOT NULL DEFAULT (datetime('now','localtime')),
  UNIQUE (document_id, alias)
);

-- ── 材料与会议的关系（未归类材料只存在于此表）────────────────────
CREATE TABLE IF NOT EXISTS meeting_documents (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  meeting_id  INTEGER NOT NULL REFERENCES meetings(id)  ON DELETE CASCADE,
  document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  folder_hint TEXT,
  added_at    TEXT    NOT NULL DEFAULT (datetime('now','localtime')),
  UNIQUE (meeting_id, document_id)
);

-- ── 议题 ──────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS topics (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  meeting_id  INTEGER NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
  order_index INTEGER NOT NULL DEFAULT 1000,
  title       TEXT    NOT NULL,
  kind        TEXT    NOT NULL DEFAULT 'department',
  -- agenda | first_item | department | other
  notes       TEXT,
  created_at  TEXT    NOT NULL DEFAULT (datetime('now','localtime')),
  updated_at  TEXT    NOT NULL DEFAULT (datetime('now','localtime'))
);

CREATE INDEX IF NOT EXISTS idx_topics_meeting ON topics (meeting_id, order_index);

-- ── 核心关系表：某份材料在某个议题里的角色 ────────────────────────
CREATE TABLE IF NOT EXISTS topic_documents (
  id                 INTEGER PRIMARY KEY AUTOINCREMENT,
  meeting_id         INTEGER NOT NULL REFERENCES meetings(id)  ON DELETE CASCADE,
  topic_id           INTEGER          REFERENCES topics(id)    ON DELETE CASCADE,
  document_id        INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  role               TEXT    NOT NULL DEFAULT 'other',
  -- agenda | first_item | main | attachment | supplement | other
  order_index        INTEGER NOT NULL DEFAULT 1000,
  attachment_label   TEXT,   -- 附件1 / 附件一 / 附件A …（用户可改）
  parent_document_id INTEGER          REFERENCES documents(id) ON DELETE SET NULL,
  origin             TEXT    NOT NULL DEFAULT 'manual',
  -- manual | auto_rule | ai_suggested_confirmed
  created_at         TEXT    NOT NULL DEFAULT (datetime('now','localtime')),
  updated_at         TEXT    NOT NULL DEFAULT (datetime('now','localtime'))
);

CREATE INDEX IF NOT EXISTS idx_topic_documents_topic
  ON topic_documents (topic_id, order_index);
CREATE INDEX IF NOT EXISTS idx_topic_documents_document
  ON topic_documents (document_id);
-- 同一议题下，同一材料同一角色只允许一条
CREATE UNIQUE INDEX IF NOT EXISTS uniq_topic_document_role
  ON topic_documents (meeting_id, ifnull(topic_id, 0), document_id, role);

-- ── 文本：每页/每节 ───────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS page_text (
  document_id    INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  page_no        INTEGER NOT NULL,
  section_label  TEXT,
  text           TEXT    NOT NULL,
  char_count     INTEGER NOT NULL DEFAULT 0,
  text_source    TEXT    NOT NULL,   -- text_layer | ocr | parsed
  ocr_confidence REAL,
  updated_at     TEXT    NOT NULL DEFAULT (datetime('now','localtime')),
  PRIMARY KEY (document_id, page_no)
);

-- ── 文本坐标块（引用跳转与高亮的定位基础）─────────────────────────
CREATE TABLE IF NOT EXISTS text_blocks (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  page_no     INTEGER NOT NULL,
  block_index INTEGER NOT NULL,
  text        TEXT    NOT NULL,
  x           REAL, y REAL, w REAL, h REAL,   -- 归一化坐标，原点左下
  confidence  REAL,
  UNIQUE (document_id, page_no, block_index)
);

CREATE INDEX IF NOT EXISTS idx_text_blocks_page ON text_blocks (document_id, page_no);

-- ── OCR 任务与缓存 ────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS ocr_jobs (
  id             INTEGER PRIMARY KEY AUTOINCREMENT,
  document_id    INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  page_no        INTEGER NOT NULL,
  engine         TEXT    NOT NULL,
  engine_version TEXT    NOT NULL DEFAULT '',
  status         TEXT    NOT NULL DEFAULT 'pending',
  error          TEXT,
  elapsed_ms     INTEGER,
  created_at     TEXT    NOT NULL DEFAULT (datetime('now','localtime')),
  updated_at     TEXT    NOT NULL DEFAULT (datetime('now','localtime')),
  UNIQUE (document_id, page_no, engine, engine_version)
);

-- ── AI 结果（永远独立存放，绝不混入原文）──────────────────────────
CREATE TABLE IF NOT EXISTS ai_results (
  id             INTEGER PRIMARY KEY AUTOINCREMENT,
  scope_type     TEXT    NOT NULL,   -- meeting | topic | document
  scope_id       INTEGER NOT NULL,
  kind           TEXT    NOT NULL,   -- summary | classify | relation | minutes
  model          TEXT    NOT NULL,
  prompt_version TEXT    NOT NULL,
  params_hash    TEXT    NOT NULL,
  content_json   TEXT    NOT NULL,
  status         TEXT    NOT NULL DEFAULT 'done',
  created_at     TEXT    NOT NULL DEFAULT (datetime('now','localtime')),
  updated_at     TEXT    NOT NULL DEFAULT (datetime('now','localtime'))
);

CREATE INDEX IF NOT EXISTS idx_ai_results_scope
  ON ai_results (scope_type, scope_id, kind);

CREATE TABLE IF NOT EXISTS ai_citations (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  ai_result_id INTEGER NOT NULL REFERENCES ai_results(id) ON DELETE CASCADE,
  document_id  INTEGER          REFERENCES documents(id)  ON DELETE SET NULL,
  page_no      INTEGER,
  quote        TEXT    NOT NULL,
  verified     INTEGER NOT NULL DEFAULT 0,   -- 0/1：是否在原文中找到
  match_score  REAL
);

CREATE INDEX IF NOT EXISTS idx_ai_citations_result ON ai_citations (ai_result_id);

-- ── 用户备注（多层级）────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS annotations (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  scope_type   TEXT    NOT NULL,   -- meeting | topic | document | page | paragraph
  scope_id     INTEGER NOT NULL,
  document_id  INTEGER          REFERENCES documents(id) ON DELETE CASCADE,
  page_no      INTEGER,
  quote        TEXT,
  anchor_start INTEGER,
  anchor_end   INTEGER,
  content      TEXT    NOT NULL,
  color        TEXT,
  created_at   TEXT    NOT NULL DEFAULT (datetime('now','localtime')),
  updated_at   TEXT    NOT NULL DEFAULT (datetime('now','localtime'))
);

CREATE INDEX IF NOT EXISTS idx_annotations_scope
  ON annotations (scope_type, scope_id);
CREATE INDEX IF NOT EXISTS idx_annotations_document
  ON annotations (document_id, page_no);

-- ── 阅读位置（恢复用）────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS reading_state (
  meeting_id  INTEGER PRIMARY KEY REFERENCES meetings(id) ON DELETE CASCADE,
  topic_id    INTEGER,
  document_id INTEGER,
  page_no     INTEGER NOT NULL DEFAULT 1,
  zoom        REAL    NOT NULL DEFAULT 1.0,
  scroll_top  REAL    NOT NULL DEFAULT 0,
  updated_at  TEXT    NOT NULL DEFAULT (datetime('now','localtime'))
);

-- ── AI 建议（确认前不影响任何结构）───────────────────────────────
CREATE TABLE IF NOT EXISTS ai_suggestions (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  meeting_id   INTEGER NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
  type         TEXT    NOT NULL,   -- topic_class | attachment_relation
  payload_json TEXT    NOT NULL,
  status       TEXT    NOT NULL DEFAULT 'pending',   -- pending | accepted | rejected
  created_at   TEXT    NOT NULL DEFAULT (datetime('now','localtime')),
  decided_at   TEXT
);

CREATE INDEX IF NOT EXISTS idx_ai_suggestions_meeting
  ON ai_suggestions (meeting_id, status);

-- ── 会后待办（第二阶段启用）──────────────────────────────────────
CREATE TABLE IF NOT EXISTS tasks (
  id                  INTEGER PRIMARY KEY AUTOINCREMENT,
  meeting_id          INTEGER NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
  topic_id            INTEGER          REFERENCES topics(id)   ON DELETE SET NULL,
  source_ai_result_id INTEGER          REFERENCES ai_results(id) ON DELETE SET NULL,
  title               TEXT    NOT NULL,
  owner_dept          TEXT,   -- 未明确时保持 NULL，界面显示"未明确责任部门"
  due_date            TEXT,   -- 未明确时保持 NULL
  status              TEXT    NOT NULL DEFAULT 'open',
  evidence_json       TEXT,
  created_at          TEXT    NOT NULL DEFAULT (datetime('now','localtime')),
  updated_at          TEXT    NOT NULL DEFAULT (datetime('now','localtime'))
);

-- ── 任务队列（导入/解析/OCR）─────────────────────────────────────
CREATE TABLE IF NOT EXISTS jobs (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  type       TEXT    NOT NULL,   -- import | parse | ocr
  payload    TEXT,
  status     TEXT    NOT NULL DEFAULT 'pending',
  progress   INTEGER NOT NULL DEFAULT 0,
  error      TEXT,
  started_at TEXT,
  finished_at TEXT,
  created_at TEXT    NOT NULL DEFAULT (datetime('now','localtime'))
);

-- ── 网络出站审计（隐私承诺的可验证依据）──────────────────────────
CREATE TABLE IF NOT EXISTS net_audit (
  id      INTEGER PRIMARY KEY AUTOINCREMENT,
  ts      TEXT    NOT NULL DEFAULT (datetime('now','localtime')),
  target  TEXT    NOT NULL,
  purpose TEXT    NOT NULL,
  allowed INTEGER NOT NULL,
  note    TEXT
);

-- ── 设置 ──────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS settings (
  key        TEXT PRIMARY KEY,
  value      TEXT NOT NULL,
  updated_at TEXT NOT NULL DEFAULT (datetime('now','localtime'))
);

-- ── 全文索引 ──────────────────────────────────────────────────────
-- trigram 分词器：中文/英文/中英混排均可做子串匹配
-- （默认 unicode61 不切分中文，会导致"预算"搜不到"关于预算的说明"）
-- rowid 约定（便于幂等重建）：
--   原文/OCR：1_000_000 * document_id + page_no
--   用户备注：1_000_000_000_000 + annotation_id
--   AI 结果：2_000_000_000_000 + ai_result_id
CREATE VIRTUAL TABLE IF NOT EXISTS search_fts USING fts5(
  document_id UNINDEXED,
  source      UNINDEXED,   -- original | ocr | note | ai
  page_no     UNINDEXED,
  body,
  tokenize = 'trigram'
);

-- ── 初始设置 ──────────────────────────────────────────────────────
INSERT OR IGNORE INTO settings (key, value) VALUES
  ('ai.enabled', 'false'),
  ('ai.autoSummary', 'false'),
  ('ai.autoClassifyTopics', 'false'),
  ('ai.autoDetectAttachments', 'false'),
  ('ai.endpoint', 'http://127.0.0.1:11434'),
  ('ai.model', ''),
  ('ocr.engine', 'apple-vision'),
  ('ocr.languages', 'zh-Hans,en-US'),
  ('ocr.concurrency', '2'),
  ('storage.libraryRoot', ''),
  ('privacy.requireDeleteConfirm', 'true');
