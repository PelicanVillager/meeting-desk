import { useState } from "react";

import { Home } from "@/app/routes/Home/Home";
import { Organize } from "@/app/routes/Organize/Organize";

/**
 * 当前有两个视图：首页工作台、整理台。
 * 阅读模式 / 会后整理 / 设置将在 M5、M9 接入。
 */
type View = { name: "home" } | { name: "organize"; meetingId: number };

export function App() {
  const [view, setView] = useState<View>({ name: "home" });

  if (view.name === "organize") {
    return (
      <Organize meetingId={view.meetingId} onBack={() => setView({ name: "home" })} />
    );
  }

  return <Home onOpenMeeting={(meetingId) => setView({ name: "organize", meetingId })} />;
}
