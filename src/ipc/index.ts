import { invoke, isTauri } from "@tauri-apps/api/core";

import type {
  AppInfo,
  DbStats,
  DeleteResult,
  DocumentRole,
  DraftResult,
  ExportResult,
  GuardDecision,
  ImportResult,
  MeetingSummary,
  MeetingTree,
  NetAuditEntry,
  OcrSelfTestResult,
  SelfCheckReport,
} from "@/domain/types";

/**
 * 与 Rust 后端的唯一入口。
 *
 * 纪律：界面层不直接触碰文件系统、数据库、网络。
 * 所有本地能力都必须经过这里的强类型命令，便于审计。
 */
export const ipc = {
  isDesktop: () => isTauri(),

  appInfo: () => invoke<AppInfo>("app_info"),

  dbStats: () => invoke<DbStats>("db_stats"),

  selfCheck: () => invoke<SelfCheckReport>("offline_self_check"),

  netAudit: (limit = 50) => invoke<NetAuditEntry[]>("net_audit_list", { limit }),

  /** 仅用于验证网络守卫：把任意地址丢进去，看是否被拒绝。 */
  guardProbe: (url: string) => invoke<GuardDecision>("guard_probe", { url }),

  /** 跑一遍本地 OCR 自测（生成测试图 → 系统 Vision 识别）。 */
  ocrSelfTest: () => invoke<OcrSelfTestResult>("ocr_self_test"),

  createMeeting: (title: string) => invoke<{ id: number; title: string }>("create_meeting", { title }),

  /** ── 会议整理 ─────────────────────────────────────────────── */

  /** 导入拖进来的文件夹（递归展开）或零散文件。 */
  importPaths: (paths: string[]) => invoke<ImportResult>("import_paths", { paths }),

  listMeetings: () => invoke<MeetingSummary[]>("list_meetings"),

  /** 删除一场会议（材料文件移到 library/trash/，可恢复） */
  deleteMeeting: (meetingId: number) =>
    invoke<DeleteResult>("delete_meeting", { meetingId }),

  meetingTree: (meetingId: number) => invoke<MeetingTree>("meeting_tree", { meetingId }),

  createTopic: (meetingId: number, title: string, department: string | null) =>
    invoke<{ topicId: number }>("create_topic", { meetingId, title, department }),

  assignDocument: (params: {
    meetingId: number;
    documentId: number;
    role: DocumentRole;
    topicId?: number | null;
    attachmentLabel?: string | null;
  }) =>
    invoke<void>("assign_document", {
      meetingId: params.meetingId,
      documentId: params.documentId,
      role: params.role,
      topicId: params.topicId ?? null,
      attachmentLabel: params.attachmentLabel ?? null,
    }),

  clearDocumentRole: (meetingId: number, documentId: number) =>
    invoke<void>("clear_document_role", { meetingId, documentId }),

  /** 按原文件夹结构生成议题草稿（部门 → 议题 → 材料） */
  autoDraftTopics: (meetingId: number) =>
    invoke<DraftResult>("auto_draft_topics", { meetingId }),

  /** 指定议题主材料，其余自动重编号为附件1..N */
  setTopicMain: (meetingId: number, topicId: number, documentId: number) =>
    invoke<void>("set_topic_main", { meetingId, topicId, documentId }),

  /** 导出单个 HTML 会议包（拷进 U 盘，会议室电脑双击即开） */
  exportBundle: (meetingId: number) => invoke<ExportResult>("export_bundle", { meetingId }),

  /** 在访达中定位文件 */
  revealPath: (path: string) => invoke<void>("reveal_path", { path }),
};
