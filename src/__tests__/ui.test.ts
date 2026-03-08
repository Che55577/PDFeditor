// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from "vitest";
import indexHtml from "../../index.html?raw";
import { getLang, GROUP_I18N, TOOL_I18N, UI_TEXT } from "../i18n";

const invokeMock = vi.fn();
const openMock = vi.fn();
const saveMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

vi.mock("@tauri-apps/api/path", () => ({
  join: vi.fn((...parts: string[]) => Promise.resolve(parts.join("/"))),
}));

vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: vi.fn(),
  }),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: (...args: unknown[]) => openMock(...args),
  save: (...args: unknown[]) => saveMock(...args),
}));

describe("LocalPDF Frontend & UI Integration Tests", () => {
  let pyextState: string | null = null;

  beforeEach(async () => {
    vi.clearAllMocks();
    pyextState = null;

    const bodyMatch = /<body>([\s\S]*?)<\/body>/i.exec(indexHtml);
    const bodyHtml = (bodyMatch ? bodyMatch[1] : indexHtml).replace(
      /<script\b[^>]*>[\s\S]*?<\/script>/gi,
      "",
    );
    document.documentElement.lang = "zh-Hant";
    document.body.innerHTML = bodyHtml;

    invokeMock.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      switch (cmd) {
        case "list_fonts":
          return Promise.resolve([{ label: "標楷體", path: "C:/Windows/Fonts/kaiu.ttf", index: 0 }]);
        case "engine_status":
          return Promise.resolve({
            ghostscript: "10.07.1",
            libreoffice: null,
            pyext: pyextState,
            qpdf: "qpdf version 12.4.2",
          });
        case "inspect": {
          const paths = (args?.paths as string[]) ?? [];
          return Promise.resolve(
            paths.map((p) => ({
              path: p,
              name: p.split(/[\\/]/).pop() ?? p,
              bytes: 102400,
              pages: 5,
              error: null,
            })),
          );
        }
        case "thumbnail":
          return Promise.resolve(new Uint8Array([0xff, 0xd8, 0xff, 0xd9]).buffer);
        case "install_pyext":
          pyextState = "Python 3.11 + pdf2docx";
          return Promise.resolve(pyextState);
        case "cancel_pipeline":
        case "discard_all":
          return Promise.resolve(null);
        default:
          return Promise.resolve(null);
      }
    });

    vi.resetModules();
    await import("../main");
    await new Promise((r) => setTimeout(r, 20));
  });

  it("verifies zh-TW and en i18n dictionary parity", () => {
    expect(Object.keys(UI_TEXT["zh-TW"]).sort()).toEqual(Object.keys(UI_TEXT.en).sort());
    for (const [, entry] of Object.entries(GROUP_I18N)) {
      expect(entry["zh-TW"].length).toBeGreaterThan(0);
      expect(entry.en.length).toBeGreaterThan(0);
    }
    for (const [, entry] of Object.entries(TOOL_I18N)) {
      expect(entry["zh-TW"][0].length).toBeGreaterThan(0);
      expect(entry.en[0].length).toBeGreaterThan(0);
    }
  });

  it("switches UI language between zh-TW and en when clicking #langToggle", () => {
    const langBtn = document.getElementById("langToggle") as HTMLButtonElement;
    const browseBtn = document.getElementById("browse") as HTMLButtonElement;
    const installBtn = document.getElementById("installPyext") as HTMLButtonElement;

    expect(getLang()).toBe("zh-TW");
    expect(browseBtn.textContent).toBe("選擇檔案");
    expect(installBtn.textContent).toBe("匯入擴充包 (.zip)");

    langBtn.click();
    expect(document.documentElement.lang).toBe("en");
    expect(browseBtn.textContent).toBe("Browse files");
    expect(installBtn.textContent).toBe("Import Extension (.zip)");

    const mergeCard = document.querySelector('.toolcard[data-tool="merge"] strong');
    expect(mergeCard?.textContent).toBe("Merge");

    langBtn.click();
    expect(document.documentElement.lang).toBe("zh-Hant");
    expect(browseBtn.textContent).toBe("選擇檔案");
  });

  it("supports keyboard accessibility (Enter and Space) on #drop zone", async () => {
    openMock.mockResolvedValueOnce(["/tmp/sample.pdf"]);
    const dropEl = document.getElementById("drop") as HTMLElement;
    expect(dropEl.getAttribute("tabindex")).toBe("0");
    expect(dropEl.getAttribute("role")).toBe("button");

    dropEl.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await new Promise((r) => setTimeout(r, 20));

    expect(openMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledWith("inspect", { paths: ["/tmp/sample.pdf"] });
  });

  it("enforces mutual exclusion rules across incompatible pipeline tools", () => {
    const checkTool = (id: string) => {
      const input = document.querySelector<HTMLInputElement>(`.toolcard[data-tool="${id}"] input`)!;
      input.checked = true;
      input.dispatchEvent(new Event("change"));
    };
    const isChecked = (id: string) =>
      document.querySelector<HTMLInputElement>(`.toolcard[data-tool="${id}"] input`)!.checked;

    // Extract vs Remove mutual exclusion
    checkTool("extract");
    expect(isChecked("extract")).toBe(true);
    checkTool("remove");
    expect(isChecked("remove")).toBe(true);
    expect(isChecked("extract")).toBe(false);

    // Terminal tools (pdftoword vs tomarkdown) and split mutual exclusion
    checkTool("split");
    expect(isChecked("split")).toBe(true);
    checkTool("pdftoword");
    expect(isChecked("pdftoword")).toBe(true);
    expect(isChecked("split")).toBe(false);

    // Exclusive tool (annotate) clears all batch selections
    checkTool("annotate");
    expect(isChecked("annotate")).toBe(true);
    expect(isChecked("remove")).toBe(false);
    expect(isChecked("pdftoword")).toBe(false);
  });

  it("installs pyext archive via #installPyext and dynamically unlocks engine status", async () => {
    const installBtn = document.getElementById("installPyext") as HTMLButtonElement;
    const engineEl = document.getElementById("engine") as HTMLElement;
    const statusEl = document.getElementById("status") as HTMLElement;

    expect(installBtn.hidden).toBe(false);
    expect(engineEl.textContent).toContain("未安裝擴充包");

    openMock.mockResolvedValueOnce("/tmp/localpdf-pyext-Windows-X64.zip");
    installBtn.click();
    await new Promise((r) => setTimeout(r, 30));

    expect(invokeMock).toHaveBeenCalledWith("install_pyext", {
      path: "/tmp/localpdf-pyext-Windows-X64.zip",
    });
    expect(statusEl.textContent).toContain("Python 3.11 + pdf2docx");
    expect(engineEl.textContent).toContain("Python 3.11 + pdf2docx");
    expect(installBtn.hidden).toBe(true);
  });

  it("invokes cancel_pipeline when clicking #cancel", () => {
    const cancelBtn = document.getElementById("cancel") as HTMLButtonElement;
    const statusEl = document.getElementById("status") as HTMLElement;

    cancelBtn.click();
    expect(invokeMock).toHaveBeenCalledWith("cancel_pipeline");
    expect(statusEl.textContent).toContain("正在取消操作");
  });
});
