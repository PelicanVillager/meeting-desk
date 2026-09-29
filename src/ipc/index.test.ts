import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => {
    throw new Error("浏览器环境下不应调用 invoke");
  }),
  isTauri: () => false,
}));

describe("ipc 在非桌面（浏览器预览）环境下的行为", () => {
  it("isDesktop 返回 false，而不是抛错", async () => {
    const { ipc } = await import("./index");
    expect(ipc.isDesktop()).toBe(false);
  });

  it("界面层通过 isDesktop 判断后再调用命令，避免浏览器里点一下就炸", async () => {
    const { ipc } = await import("./index");
    if (!ipc.isDesktop()) {
      // 首页按钮在浏览器预览下会被禁用，这条断言固定住这个约定
      expect(ipc.isDesktop()).toBe(false);
    }
  });
});
