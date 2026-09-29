import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";

import type {
  AppInfo,
  DbStats,
  MeetingSummary,
  NetAuditEntry,
  OcrSelfTestResult,
  SelfCheckReport,
} from "@/domain/types";
import { ipc } from "@/ipc";
import { formatBytes } from "@/utils/format";
import styles from "./Home.module.css";

interface Props {
  onOpenMeeting: (meetingId: number) => void;
}

export function Home({ onOpenMeeting }: Props) {
  const [appInfo, setAppInfo] = useState<AppInfo | null>(null);
  const [stats, setStats] = useState<DbStats | null>(null);
  const [meetings, setMeetings] = useState<MeetingSummary[]>([]);
  const [report, setReport] = useState<SelfCheckReport | null>(null);
  const [audit, setAudit] = useState<NetAuditEntry[]>([]);
  const [ocrResult, setOcrResult] = useState<OcrSelfTestResult | null>(null);
  const [dragging, setDragging] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmingId, setConfirmingId] = useState<number | null>(null);
  const isDesktop = useRef(ipc.isDesktop()).current;

  const refresh = useCallback(async () => {
    if (!isDesktop) return;
    try {
      const [info, dbStats, auditList, meetingList] = await Promise.all([
        ipc.appInfo(),
        ipc.dbStats(),
        ipc.netAudit(20),
        ipc.listMeetings(),
      ]);
      setAppInfo(info);
      setStats(dbStats);
      setAudit(auditList);
      setMeetings(meetingList);
      setError(null);
    } catch (err) {
      setError(String(err));
    }
  }, [isDesktop]);

  const importPaths = useCallback(
    async (paths: string[]) => {
      setBusy("导入");
      setError(null);
      setNotice(null);
      try {
        const result = await ipc.importPaths(paths);
        setNotice(
          `已导入《${result.meetingTitle}》：${result.files.length} 份材料` +
            (result.duplicateCount > 0
              ? `（其中 ${result.duplicateCount} 份是库里已有的内容，未重复占用空间）`
              : "") +
            (result.skipped.length > 0 ? `，跳过 ${result.skipped.length} 个无关文件` : ""),
        );
        onOpenMeeting(result.meetingId);
      } catch (err) {
        setError(String(err));
      } finally {
        setBusy(null);
      }
    },
    [onOpenMeeting],
  );

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // 桌面应用：Tauri 原生拖放，能拿到真实本地路径
  useEffect(() => {
    if (!isDesktop) return;
    let unlisten: (() => void) | undefined;
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === "over") {
          setDragging(true);
          return;
        }
        setDragging(false);
        if (event.payload.type === "drop") {
          const paths: string[] = event.payload.paths;
          if (paths.length > 0) void importPaths(paths);
        }
      })
      .then((fn) => {
        unlisten = fn;
      });
    return () => unlisten?.();
  }, [importPaths, isDesktop]);

  const run = async (label: string, task: () => Promise<void>) => {
    setBusy(label);
    setError(null);
    try {
      await task();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(null);
    }
  };

  const removeMeeting = (meeting: MeetingSummary) =>
    run("删除", async () => {
      const result = await ipc.deleteMeeting(meeting.meetingId);
      setConfirmingId(null);
      setNotice(
        `已删除《${result.meetingTitle}》` +
          (result.documentsRemoved > 0
            ? `，${result.documentsRemoved} 份材料、${result.blobsMovedToTrash} 个文件已移入回收区` +
              `（${formatBytes(result.bytesFreed)}，可在库里的 library/trash 找回）`
            : "（这场会议没有材料）"),
      );
      await refresh();
    });

  return (
    <div className={styles.page}>
      <header className={styles.header}>
        <div className={styles.brand}>会议材料工作台</div>
        <div className={styles.localBadge} title={appInfo?.libraryRoot ?? ""}>
          🔒 本地模式
        </div>
        <div className={styles.headerRight}>
          <span className={styles.muted}>
            {appInfo ? `本地AI：${appInfo.aiConfigured ? "已连接" : "未配置"}` : "…"}
          </span>
          <span className={styles.muted}>
            {appInfo ? `OCR：${appInfo.ocrReady ? appInfo.ocrEngine : "未就绪"}` : "…"}
          </span>
        </div>
      </header>

      <main className={styles.main}>
        <section className={`${styles.dropzone} ${dragging ? styles.dropzoneActive : ""}`}>
          <div className={styles.dropIcon}>📁</div>
          <div className={styles.dropTitle}>
            {busy === "导入" ? "正在导入…" : "把整场会议的文件夹拖进来"}
          </div>
          <div className={styles.dropHint}>
            拖文件夹会递归展开；也可以一次拖多个文件
          </div>
          <div className={styles.dropNote}>文件不会上传到云端</div>
        </section>

        {notice && <div className={styles.notice}>{notice}</div>}
        {error && <div className={styles.error}>出错：{error}</div>}
        {!isDesktop && (
          <p className={styles.note}>
            当前在浏览器里预览界面，本地能力不可用。请用 <code>pnpm app:dev</code> 启动桌面应用。
          </p>
        )}

        <section className={styles.panel}>
          <div className={styles.panelTitle}>
            最近会议
            {stats && (
              <span className={styles.muted}>
                共 {stats.meetings} 场 · 材料 {stats.documents} 份 · 占用{" "}
                {formatBytes(stats.blobBytes)}
              </span>
            )}
          </div>
          {meetings.length === 0 ? (
            <p className={styles.note}>还没有会议。把文件夹拖到上面的区域即可开始。</p>
          ) : (
            <ul className={styles.meetingList}>
              {meetings.map((meeting) => (
                <li key={meeting.meetingId}>
                  <div className={styles.meetingRowWrap}>
                    <button
                      className={styles.meetingRow}
                      onClick={() => onOpenMeeting(meeting.meetingId)}
                    >
                      <span className={styles.meetingDate}>
                        {meeting.meetingDate ?? meeting.createdAt.slice(0, 10)}
                      </span>
                      <span className={styles.meetingTitle}>{meeting.title}</span>
                      <span className={styles.muted}>{meeting.documentCount} 份材料</span>
                      <span className={styles.openHint}>整理 →</span>
                    </button>

                    {confirmingId === meeting.meetingId ? (
                      <span className={styles.confirmBar}>
                        <span className={styles.confirmText}>
                          删除这场会议？
                          {meeting.documentCount > 0
                            ? `（${meeting.documentCount} 份材料会移到回收区，可恢复）`
                            : "（这场会议没有材料）"}
                        </span>
                        <button
                          className={styles.dangerBtn}
                          disabled={busy !== null}
                          onClick={() => void removeMeeting(meeting)}
                        >
                          确认删除
                        </button>
                        <button className={styles.smallBtn} onClick={() => setConfirmingId(null)}>
                          取消
                        </button>
                      </span>
                    ) : (
                      <button
                        className={styles.deleteBtn}
                        title="删除这场会议"
                        onClick={() => setConfirmingId(meeting.meetingId)}
                      >
                        删除
                      </button>
                    )}
                  </div>
                </li>
              ))}
            </ul>
          )}
        </section>

        <section className={styles.panel}>
          <div className={styles.panelTitle}>自检与状态</div>
          <div className={styles.actions}>
            <button
              onClick={() =>
                run("自检", async () => {
                  setReport(await ipc.selfCheck());
                  setAudit(await ipc.netAudit(20));
                })
              }
              disabled={!isDesktop || busy !== null}
            >
              {busy === "自检" ? "检查中…" : "运行离线自检"}
            </button>
            <button
              onClick={() =>
                run("OCR", async () => {
                  setOcrResult(await ipc.ocrSelfTest());
                })
              }
              disabled={!isDesktop || busy !== null}
            >
              {busy === "OCR" ? "识别中…" : "测试本地 OCR"}
            </button>
            <button
              onClick={() =>
                run("网络守卫", async () => {
                  await ipc.guardProbe("https://example.com/upload");
                  setAudit(await ipc.netAudit(20));
                })
              }
              disabled={!isDesktop || busy !== null}
            >
              试探公网地址（应被拒绝）
            </button>
          </div>

          {report && (
            <table className={styles.table}>
              <tbody>
                {report.items.map((item) => (
                  <tr key={item.id}>
                    <td className={styles.statusCell}>
                      <span className={item.ok ? styles.okMark : styles.badMark}>
                        {item.ok ? "✓" : "✕"}
                      </span>
                    </td>
                    <td>{item.label}</td>
                    <td className={styles.detailCell}>{item.detail}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}

          {ocrResult && (
            <div className={styles.ocrBox}>
              <div className={styles.panelSubTitle}>
                本地 OCR 自测：{ocrResult.ok ? "通过" : "失败"}　引擎 {ocrResult.engine}{" "}
                {ocrResult.engineVersion}　耗时 {ocrResult.elapsedMs} ms　文本块{" "}
                {ocrResult.blockCount}
              </div>
              <pre className={styles.pre}>{ocrResult.recognizedText || ocrResult.error}</pre>
            </div>
          )}

          {audit.length > 0 && (
            <details className={styles.details}>
              <summary>网络出站审计（{audit.length} 条）</summary>
              <table className={styles.table}>
                <tbody>
                  {audit.map((entry) => (
                    <tr key={entry.id}>
                      <td className={styles.detailCell}>{entry.ts}</td>
                      <td className={styles.detailCell}>{entry.target}</td>
                      <td>{entry.purpose}</td>
                      <td>
                        <span className={entry.allowed ? styles.okMark : styles.badMark}>
                          {entry.allowed ? "允许" : "已拒绝"}
                        </span>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </details>
          )}
        </section>
      </main>

      <footer className={styles.footer}>
        <span>文件仅保存在本机，不会上传云端。</span>
        <span className={styles.footerRight}>
          {appInfo ? `库位置：${appInfo.libraryRoot}` : ""}
        </span>
      </footer>
    </div>
  );
}
