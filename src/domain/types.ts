/** 与 Rust 侧 serde 结构一一对应（Rust 侧统一 rename_all = "camelCase"）。 */

export interface AppInfo {
  version: string;
  localMode: true;
  libraryRoot: string;
  dbPath: string;
  ocrEngine: string;
  ocrReady: boolean;
  aiConfigured: boolean;
}

export interface DbStats {
  meetings: number;
  documents: number;
  blobs: number;
  blobBytes: number;
  annotations: number;
  schemaVersion: number;
}

export interface SelfCheckItem {
  id: string;
  label: string;
  ok: boolean;
  detail: string;
}

export interface SelfCheckReport {
  ok: boolean;
  checkedAt: string;
  items: SelfCheckItem[];
}

export interface NetAuditEntry {
  id: number;
  ts: string;
  target: string;
  purpose: string;
  allowed: boolean;
  note: string | null;
}

export interface GuardDecision {
  target: string;
  allowed: boolean;
  reason: string;
}

export interface OcrBlock {
  text: string;
  confidence: number;
  /** 归一化坐标，原点在左下角（与 Vision 框架一致）。 */
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface OcrSelfTestResult {
  ok: boolean;
  engine: string;
  engineVersion: string;
  elapsedMs: number;
  recognizedText: string;
  matchedKeyPhrase: boolean;
  blockCount: number;
  error: string | null;
}

/** ── 会议整理（导入 / 角色指定 / 结构）────────────────────────────── */

export interface ImportedFile {
  documentId: number;
  filename: string;
  displayName: string;
  ext: string;
  size: number;
  folderHint: string | null;
  isDuplicate: boolean;
}

export interface ImportResult {
  meetingId: number;
  meetingTitle: string;
  meetingDate: string | null;
  files: ImportedFile[];
  skipped: string[];
  duplicateCount: number;
}

export interface TreeDocument {
  documentId: number;
  displayName: string;
  filename: string;
  ext: string;
  size: number;
  role: string;
  attachmentLabel: string | null;
  folderHint: string | null;
  orderIndex: number;
}

export interface TopicNode {
  topicId: number;
  title: string;
  department: string | null;
  orderIndex: number;
  main: TreeDocument | null;
  attachments: TreeDocument[];
}

export interface MeetingTree {
  meetingId: number;
  title: string;
  meetingDate: string | null;
  agenda: TreeDocument | null;
  topics: TopicNode[];
  unassigned: TreeDocument[];
  others: TreeDocument[];
  totalDocuments: number;
}

export interface MeetingSummary {
  meetingId: number;
  title: string;
  meetingDate: string | null;
  documentCount: number;
  createdAt: string;
}

/** 材料在会议里的角色（与 Rust 侧取值一致）。 */
export type DocumentRole = "agenda" | "main" | "attachment" | "other";

export interface DraftTopic {
  topicId: number;
  title: string;
  department: string | null;
  documentCount: number;
}

export interface DraftResult {
  topicsCreated: number;
  documentsGrouped: number;
  leftUnassigned: number;
  topics: DraftTopic[];
}

export interface ExportResult {
  outPath: string;
  bytes: number;
  documentCount: number;
  pdfPages: number;
  warnings: string[];
}

export interface DeleteResult {
  meetingTitle: string;
  documentsRemoved: number;
  blobsMovedToTrash: number;
  bytesFreed: number;
  trashDir: string | null;
}
