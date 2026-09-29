import { useCallback, useEffect, useMemo, useState } from "react";

import type {
  DocumentRole,
  ExportResult,
  MeetingTree,
  TopicNode,
  TreeDocument,
} from "@/domain/types";
import { ipc } from "@/ipc";
import { formatBytes, stripExtension } from "@/utils/format";
import styles from "./Organize.module.css";

interface Props {
  meetingId: number;
  onBack: () => void;
}

interface Assignment {
  role: string;
  topicTitle: string | null;
  label: string | null;
}

export function Organize({ meetingId, onBack }: Props) {
  const [tree, setTree] = useState<MeetingTree | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [exported, setExported] = useState<ExportResult | null>(null);
  const [busy, setBusy] = useState(false);
  const [filter, setFilter] = useState("");

  const reload = useCallback(async () => {
    try {
      setTree(await ipc.meetingTree(meetingId));
      setError(null);
    } catch (err) {
      setError(String(err));
    }
  }, [meetingId]);

  useEffect(() => {
    void reload();
  }, [reload]);

  /** 每份材料当前的角色（用于在文件行上显示"已归到哪里"） */
  const assignments = useMemo(() => {
    const map = new Map<number, Assignment>();
    if (!tree) return map;
    if (tree.agenda) {
      map.set(tree.agenda.documentId, { role: "agenda", topicTitle: null, label: null });
    }
    for (const topic of tree.topics) {
      if (topic.main) {
        map.set(topic.main.documentId, {
          role: "main",
          topicTitle: topic.title,
          label: null,
        });
      }
      for (const item of topic.attachments) {
        map.set(item.documentId, {
          role: "attachment",
          topicTitle: topic.title,
          label: item.attachmentLabel,
        });
      }
    }
    for (const item of tree.others) {
      map.set(item.documentId, { role: "other", topicTitle: null, label: null });
    }
    return map;
  }, [tree]);

  /** 待整理的材料：未归类 + 已标其他（其他材料也要能重新归类） */
  const pool = useMemo(() => {
    if (!tree) return [];
    const list = [...tree.unassigned, ...tree.others];
    return list.sort(
      (a, b) =>
        (a.folderHint ?? "").localeCompare(b.folderHint ?? "", "zh") ||
        a.displayName.localeCompare(b.displayName, "zh"),
    );
  }, [tree]);

  const visible = useMemo(() => {
    const needle = filter.trim().toLowerCase();
    if (!needle) return pool;
    return pool.filter(
      (item) =>
        item.displayName.toLowerCase().includes(needle) ||
        (item.folderHint ?? "").toLowerCase().includes(needle),
    );
  }, [pool, filter]);

  const run = async (task: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await task();
      await reload();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  /** 按原文件夹结构生成议题草稿：部门 → 议题 → 材料 */
  const draft = () =>
    run(async () => {
      const result = await ipc.autoDraftTopics(meetingId);
      if (result.topicsCreated === 0) {
        setNotice(
          `没有可自动归类的材料。还有 ${result.leftUnassigned} 份（根目录或部门直属的文件，例如议程总表）需要手动指定角色。`,
        );
        return;
      }
      setNotice(
        `已按目录生成 ${result.topicsCreated} 个议题草稿、归入 ${result.documentsGrouped} 份材料。` +
          (result.leftUnassigned > 0
            ? `还有 ${result.leftUnassigned} 份在根目录或部门直属，需要手动指定角色。`
            : "") +
          `接下来在每个议题里点一下「设为主材料」即可。`,
      );
    });

  /** 导出单个 HTML 会议包：拷进 U 盘，会议室电脑双击即开 */
  const exportBundle = () =>
    run(async () => {
      const result = await ipc.exportBundle(meetingId);
      setExported(result);
      setNotice(
        `已导出会议包：${result.documentCount} 份材料` +
          (result.pdfPages > 0 ? `、${result.pdfPages} 页 PDF` : "") +
          `，共 ${formatBytes(result.bytes)}。` +
          (result.warnings.length > 0 ? `（${result.warnings.length} 条提示）` : ""),
      );
    });

  const topics: TopicNode[] = tree?.topics ?? [];

  return (
    <div className={styles.page}>
      <header className={styles.header}>
        <button className={styles.back} onClick={onBack}>
          ← 返回
        </button>
        <div className={styles.title}>
          {tree?.title ?? "载入中…"}
          {tree?.meetingDate ? <span className={styles.date}>{tree.meetingDate}</span> : null}
        </div>
        <div className={styles.headerRight}>
          <button
            className={styles.primary}
            disabled={busy}
            onClick={() => void draft()}
            title="按原文件夹的「部门 / 议题」层级自动建立议题，并把材料挂进去"
          >
            {busy ? "处理中…" : "按目录生成议题草稿"}
          </button>
          <button
            disabled={busy}
            onClick={() => void exportBundle()}
            title="把整场会议打包成一个 HTML 文件，拷进 U 盘，会议室电脑双击即可打开"
          >
            导出为单个 HTML
          </button>
          <span className={styles.muted}>共 {tree?.totalDocuments ?? 0} 份材料</span>
          <span className={styles.localBadge}>🔒 本地</span>
        </div>
      </header>

      {notice && <div className={styles.notice}>{notice}</div>}
      {exported && (
        <div className={styles.exportBox}>
          <div className={styles.exportPath}>{exported.outPath}</div>
          <button
            className={styles.smallBtn}
            onClick={() => void ipc.revealPath(exported.outPath)}
          >
            在访达中显示
          </button>
          {exported.warnings.length > 0 && (
            <ul className={styles.warnList}>
              {exported.warnings.slice(0, 5).map((warning) => (
                <li key={warning}>{warning}</li>
              ))}
            </ul>
          )}
        </div>
      )}
      {error && <div className={styles.error}>{error}</div>}

      <main className={styles.main}>
        <section className={styles.panel}>
          <div className={styles.panelHead}>
            <span>待整理的材料（{pool.length}）</span>
            <input
              className={styles.search}
              value={filter}
              onChange={(event) => setFilter(event.target.value)}
              placeholder="过滤文件名 / 部门"
            />
          </div>

          {pool.length === 0 ? (
            <p className={styles.empty}>所有材料都已归类。</p>
          ) : (
            <ul className={styles.fileList}>
              {visible.map((item) => (
                <FileRow
                  key={item.documentId}
                  document={item}
                  topics={topics}
                  busy={busy}
                  onAssign={(role, topicId, label) =>
                    run(() =>
                      ipc.assignDocument({
                        meetingId,
                        documentId: item.documentId,
                        role,
                        topicId: topicId ?? null,
                        attachmentLabel: label ?? null,
                      }),
                    )
                  }
                  onCreateTopic={async (title, department) => {
                    const created = await ipc.createTopic(
                      meetingId,
                      title,
                      department.trim() === "" ? null : department.trim(),
                    );
                    return created.topicId;
                  }}
                />
              ))}
            </ul>
          )}
        </section>

        <section className={styles.panel}>
          <div className={styles.panelHead}>
            <span>会议结构</span>
            <span className={styles.hint}>左边选角色，这里实时成形</span>
          </div>

          <div className={styles.agenda}>
            <span className={styles.roleTag}>议程</span>
            {tree?.agenda ? (
              <span className={styles.agendaFile}>
                📋 {tree.agenda.displayName}.{tree.agenda.ext}
              </span>
            ) : (
              <span className={styles.empty}>尚未指定——把左上那份"会议议程/议题总表"设为议程</span>
            )}
          </div>

          {topics.length === 0 ? (
            <p className={styles.empty}>还没有议题。给某份材料选"设为议题主材料"即可建立第一个议题。</p>
          ) : (
            <ol className={styles.topicList}>
              {topics.map((topic, index) => (
                <li key={topic.topicId} className={styles.topic}>
                  <div className={styles.topicHead}>
                    <span className={styles.topicIndex}>{String(index + 1).padStart(2, "0")}</span>
                    <span className={styles.topicTitle}>{topic.title}</span>
                    {topic.department && (
                      <span className={styles.department}>{topic.department}</span>
                    )}
                  </div>

                  <div className={styles.topicBody}>
                    {topic.main ? (
                      <div className={styles.mainRow}>
                        <span className={styles.roleTag}>主材料</span>
                        <span>
                          📄 {topic.main.displayName}.{topic.main.ext}
                        </span>
                        <div className={styles.rowActions}>
                          <button
                            className={styles.smallBtn}
                            disabled={busy}
                            onClick={() =>
                              run(() => ipc.clearDocumentRole(meetingId, topic.main!.documentId))
                            }
                          >
                            移出
                          </button>
                        </div>
                      </div>
                    ) : (
                      <div className={styles.missing}>📄 还没有主材料</div>
                    )}

                    {topic.attachments.map((item) => (
                      <div key={item.documentId} className={styles.attachmentRow}>
                        <span className={styles.attachmentLabel}>
                          📎 {item.attachmentLabel ?? "附件"}
                        </span>
                        <span>
                          {item.displayName}.{item.ext}
                        </span>
                        <div className={styles.rowActions}>
                          <button
                            className={`${styles.smallBtn} ${topic.main ? "" : styles.alwaysVisible}`}
                            disabled={busy}
                            onClick={() =>
                              run(() =>
                                ipc.setTopicMain(meetingId, topic.topicId, item.documentId),
                              )
                            }
                          >
                            设为主材料
                          </button>
                          <button
                            className={styles.smallBtn}
                            disabled={busy}
                            onClick={() =>
                              run(() => ipc.clearDocumentRole(meetingId, item.documentId))
                            }
                          >
                            移出
                          </button>
                        </div>
                      </div>
                    ))}
                    {topic.attachments.length === 0 && (
                      <div className={styles.missing}>📎 还没有附件</div>
                    )}
                  </div>
                </li>
              ))}
            </ol>
          )}

          {assignments.size === 0 && tree && tree.totalDocuments > 0 && (
            <p className={styles.hint}>提示：材料顺序按原文件夹排列，与纸质件顺序基本一致。</p>
          )}
        </section>
      </main>
    </div>
  );
}

interface FileRowProps {
  document: TreeDocument;
  topics: TopicNode[];
  busy: boolean;
  onAssign: (role: DocumentRole, topicId?: number | null, label?: string | null) => void;
  onCreateTopic: (title: string, department: string) => Promise<number>;
}

function FileRow({ document, topics, busy, onAssign, onCreateTopic }: FileRowProps) {
  const [mode, setMode] = useState<"main" | "attachment" | null>(null);
  const [topicTitle, setTopicTitle] = useState(stripExtension(document.displayName));
  const [department, setDepartment] = useState(document.folderHint ?? "");
  const [targetTopic, setTargetTopic] = useState<string>("new");
  const [newTopicTitle, setNewTopicTitle] = useState(stripExtension(document.displayName));
  const [newTopicDept, setNewTopicDept] = useState(document.folderHint ?? "");
  const [label, setLabel] = useState("");
  const [localError, setLocalError] = useState<string | null>(null);

  const handleSelect = (value: string) => {
    setLocalError(null);
    if (value === "agenda") {
      onAssign("agenda");
      return;
    }
    if (value === "other") {
      onAssign("other");
      return;
    }
    if (value === "main" || value === "attachment") {
      setMode(value);
      return;
    }
    setMode(null);
  };

  const submitMain = async () => {
    if (topicTitle.trim() === "") {
      setLocalError("议题名称不能为空");
      return;
    }
    try {
      const topicId = await onCreateTopic(topicTitle, department);
      onAssign("main", topicId, null);
      setMode(null);
    } catch (err) {
      setLocalError(String(err));
    }
  };

  const submitAttachment = async () => {
    try {
      let topicId: number;
      if (targetTopic === "new") {
        if (newTopicTitle.trim() === "") {
          setLocalError("新建议题需要填名称");
          return;
        }
        topicId = await onCreateTopic(newTopicTitle, newTopicDept);
      } else {
        topicId = Number(targetTopic);
      }
      onAssign("attachment", topicId, label.trim() === "" ? null : label.trim());
      setMode(null);
    } catch (err) {
      setLocalError(String(err));
    }
  };

  return (
    <li className={styles.fileRow}>
      <div className={styles.fileMain}>
        <span className={styles.fileName}>{document.displayName}</span>
        <span className={styles.fileMeta}>
          {document.ext.toUpperCase()}
          {` · ${formatBytes(document.size)}`}
          {document.folderHint ? ` · ${document.folderHint}` : ""}
        </span>
        <select
          className={styles.roleSelect}
          value=""
          disabled={busy}
          onChange={(event) => handleSelect(event.target.value)}
        >
          <option value="">选择角色…</option>
          <option value="agenda">设为会议议程</option>
          <option value="main">设为议题主材料</option>
          <option value="attachment">设为议题附件</option>
          <option value="other">设为其他材料</option>
        </select>
      </div>

      {mode === "main" && (
        <div className={styles.form}>
          <label className={styles.field}>
            <span>议题名称</span>
            <input
              value={topicTitle}
              onChange={(event) => setTopicTitle(event.target.value)}
              placeholder="例如：关于××事项的请示"
            />
          </label>
          <label className={styles.field}>
            <span>部门</span>
            <input
              value={department}
              onChange={(event) => setDepartment(event.target.value)}
              placeholder="例如：人事人才部"
            />
          </label>
          <div className={styles.formActions}>
            <button className={styles.primary} disabled={busy} onClick={() => void submitMain()}>
              建立议题并归入
            </button>
            <button className={styles.smallBtn} onClick={() => setMode(null)}>
              取消
            </button>
          </div>
        </div>
      )}

      {mode === "attachment" && (
        <div className={styles.form}>
          <label className={styles.field}>
            <span>归属议题</span>
            <select
              value={targetTopic}
              onChange={(event) => setTargetTopic(event.target.value)}
            >
              <option value="new">＋ 新建议题</option>
              {topics.map((topic) => (
                <option key={topic.topicId} value={String(topic.topicId)}>
                  {topic.title}
                </option>
              ))}
            </select>
          </label>

          {targetTopic === "new" ? (
            <>
              <label className={styles.field}>
                <span>新议题名称</span>
                <input
                  value={newTopicTitle}
                  onChange={(event) => setNewTopicTitle(event.target.value)}
                />
              </label>
              <label className={styles.field}>
                <span>部门</span>
                <input
                  value={newTopicDept}
                  onChange={(event) => setNewTopicDept(event.target.value)}
                />
              </label>
            </>
          ) : null}

          <label className={styles.field}>
            <span>附件编号</span>
            <input
              value={label}
              onChange={(event) => setLabel(event.target.value)}
              placeholder="留空自动编号（附件1、附件2…）"
            />
          </label>
          <div className={styles.formActions}>
            <button
              className={styles.primary}
              disabled={busy}
              onClick={() => void submitAttachment()}
            >
              挂为附件
            </button>
            <button className={styles.smallBtn} onClick={() => setMode(null)}>
              取消
            </button>
          </div>
        </div>
      )}

      {localError && <div className={styles.rowError}>{localError}</div>}
    </li>
  );
}
