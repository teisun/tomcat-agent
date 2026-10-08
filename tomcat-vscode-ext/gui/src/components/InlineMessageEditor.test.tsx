import "@testing-library/jest-dom/vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeAll, describe, expect, it, vi } from "vitest";
import { InlineMessageEditor } from "./InlineMessageEditor";
import { ComposerSurface, type ComposerSurfaceProps } from "./ComposerSurface";
import { messageIdsWithWritesAfter } from "./checkpointMarkers";
import type { WebviewIntent, WebviewTimelineItem } from "../types";

beforeAll(() => {
  Object.defineProperty(Document.prototype, "elementFromPoint", { configurable: true, value: () => document.body });
  Object.defineProperty(HTMLElement.prototype, "scrollIntoView", { configurable: true, value: vi.fn() });
  Object.defineProperty(Range.prototype, "getClientRects", {configurable:true,value:()=>[]});
  Object.defineProperty(Range.prototype, "getBoundingClientRect", {configurable:true,value:()=>({top:0,bottom:0,left:0,right:0,width:0,height:0,x:0,y:0,toJSON:()=>({})})});
});

const noop = () => undefined;
const props: ComposerSurfaceProps = {
  availableModels:["model"], canInterrupt:false,canPrompt:true,busy:false,contextSearchLoading:false,
  contextSearchMatches:[],contextSearchQuery:"",contextSearchTruncated:false,contextLabel:"Ctx",modeValue:"chat",modelValue:"model",thinkingLevelValue:"",
  onContextSearchClose:noop,onContextSearchOpen:noop,onContextSearchQueryChange:noop,onContextWindowChange:noop,
  onDraftChange:noop,onModeChange:noop,onModelChange:noop,onPickContext:noop,onResolveDrop:noop,onThinkingLevelChange:noop,
  onSpeedChange:noop,onSubmit:noop,attachments:[],onOpenAttachment:noop,onRemoveAttachment:noop,
};
const message = {type:"message" as const, kind:"user" as const, id:"old", text:"Original message", rewindEligible:true};
function setup(hasWrites=false, busy=false, overrides: Partial<ComposerSurfaceProps> = {}) {
  const postMessage = vi.fn();const onClose=vi.fn();
  render(<InlineMessageEditor message={message} sessionId="session" composerProps={{...props, ...overrides}} hasWrites={hasWrites} busy={busy} vscodeApi={{postMessage}} onClose={onClose}/>);
  const intents=()=>postMessage.mock.calls.map(([intent])=>intent as WebviewIntent);
  return {postMessage,onClose,intents};
}
async function reply(requestId:string, content:Record<string,unknown>) {
  await act(async()=>window.dispatchEvent(new MessageEvent("message",{data:{channel:"event",messageId:"result",content:{sessionId:"session",requestId,...content}}})));
}

describe("inline user-message edit",()=>{
  it("locks configuration to the running session without disabling the historical draft", async () => {
    const onModeChange = vi.fn(), onModelChange = vi.fn();
    const attached = { ...message, attachments: [{ id: "image", blobSha: "a".repeat(64), filename: "image.png", kind: "image" as const, mimeType: "image/png" }] };
    const renderEditor = (busy: boolean) => <InlineMessageEditor message={attached} sessionId="session"
      composerProps={{ ...props, onModeChange, onModelChange }} hasWrites={false} busy={busy}
      vscodeApi={{ postMessage: vi.fn() }} onClose={vi.fn()} />;
    const view = render(renderEditor(true));
    const input = await screen.findByTestId("edit-composer-input");
    expect(input).toHaveAttribute("contenteditable", "true");
    expect(screen.getByTestId("edit-attachment-add")).not.toBeDisabled();
    expect(screen.getByTestId("edit-mode-select")).toBeDisabled();
    expect(screen.getByTestId("edit-model-select")).toBeDisabled();
    expect(screen.getByTestId("edit-send-button")).not.toBeDisabled();
    expect(screen.queryByTestId("edit-stop-button")).toBeNull();
    fireEvent.click(screen.getByTestId("edit-mode-select"));
    fireEvent.click(screen.getByTestId("edit-model-select"));
    expect(onModeChange).not.toHaveBeenCalled(); expect(onModelChange).not.toHaveBeenCalled();
    await act(async () => { fireEvent.paste(input, { clipboardData: { getData: () => " amended" } }); });
    const edited = input.textContent;
    view.rerender(renderEditor(false));
    expect(screen.getByTestId("edit-composer-input")).toBe(input);
    expect(input.textContent).toBe(edited);
    expect(screen.getByRole("list", { name: "Pending attachments" })).toBeInTheDocument();
    expect(screen.getByTestId("edit-mode-select")).not.toBeDisabled();
    expect(screen.getByTestId("edit-model-select")).not.toBeDisabled();
    fireEvent.click(screen.getByTestId("edit-mode-select"));
    fireEvent.click(screen.getAllByTestId("edit-mode-option").find(option => option.textContent === "Plan")!);
    expect(onModeChange).toHaveBeenCalledWith("plan");
  });

  it("does not resend an attachment without text", async () => {
    const postMessage = vi.fn();
    render(<InlineMessageEditor
      message={{ ...message, text: "", attachments: [{ id: "image", blobSha: "a".repeat(64), filename: "image.png", kind: "image", mimeType: "image/png" }] }}
      sessionId="session" composerProps={props} hasWrites={false} vscodeApi={{ postMessage }} onClose={vi.fn()}
    />);
    await waitFor(() => expect(screen.getByTestId("edit-composer-input").textContent).toBe(""));
    expect(screen.getByTestId("edit-send-button")).toBeDisabled();
    fireEvent.click(screen.getByTestId("edit-send-button"));
    expect(postMessage).not.toHaveBeenCalled();
  });

  it("hydrates the full composer and sends Keep immediately without writes",async()=>{
    const {intents}=setup();
    await waitFor(()=>expect(screen.getByTestId("edit-composer-input").textContent).toBe("Original message"));
    fireEvent.click(screen.getByTestId("edit-send-button"));
    expect(intents()).toContainEqual(expect.objectContaining({type:"rewindAndResend",data:expect.objectContaining({messageId:"old",files:"keep",text:"Original message"})}));
    expect(intents().some((i)=>i.type==="previewRewind")).toBe(false);
    expect(screen.queryByRole("dialog")).toBeNull();
  });
  it("previews writes, preserves the draft on Cancel, and respects disabled Revert",async()=>{
    const {intents,onClose}=setup(true);
    await waitFor(()=>expect(screen.getByTestId("edit-send-button")).not.toBeDisabled());
    fireEvent.click(screen.getByTestId("edit-send-button"));
    const request=intents().find((i)=>i.type==="previewRewind")!;
    await reply(request.messageId,{type:"previewRewindResult",success:true,preview:{revertAvailable:false,revertReason:"no_baselines",revertPaths:[]}});
    expect(screen.getByTestId("edit-confirm-revert")).toBeDisabled();
    fireEvent.keyDown(document,{key:"Enter"});
    expect(intents().some((i)=>i.type==="rewindAndResend")).toBe(false);
    fireEvent.click(screen.getByTestId("edit-confirm-cancel"));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByTestId("edit-composer-input").textContent).toBe("Original message");
    expect(onClose).not.toHaveBeenCalled();
  });
  it.each(["keep", "cancel", "revert"] as const)("Enter activates the focused %s action, not an unintended revert", async (action) => {
    const {intents} = setup(true);
    await waitFor(()=>expect(screen.getByTestId("edit-send-button")).not.toBeDisabled());
    fireEvent.click(screen.getByTestId("edit-send-button"));
    const request=intents().find((i)=>i.type==="previewRewind")!;
    await reply(request.messageId,{type:"previewRewindResult",success:true,preview:{revertAvailable:true,revertPaths:["a.txt"]}});
    fireEvent.keyDown(document,{key:"Enter",isComposing:true});
    expect(intents().some((i)=>i.type==="rewindAndResend")).toBe(false);
    const button=screen.getByTestId(`edit-confirm-${action}`);
    button.focus(); fireEvent.keyDown(button,{key:"Enter"});
    const sends=intents().filter((i)=>i.type==="rewindAndResend");
    if(action==="cancel") expect(sends).toHaveLength(0);
    else expect(sends).toEqual([expect.objectContaining({data:expect.objectContaining({files:action})})]);
  });
  it.each([
    ["no_baselines", "这条消息没有可用的文件备份"],
    ["expired", "这条消息的文件备份已过期"],
    ["git_head_moved", "这条消息之后有过 Git 提交或切换过分支"],
  ])("uses plain copy for unavailable Revert: %s", async (reason, text) => {
    const {intents} = setup(true);
    await waitFor(() => expect(screen.getByTestId("edit-send-button")).not.toBeDisabled());
    fireEvent.click(screen.getByTestId("edit-send-button"));
    await reply(intents().find((i) => i.type === "previewRewind")!.messageId, {
      type: "previewRewindResult", success: true, preview: { revertAvailable: false, revertReason: reason, revertPaths: [] },
    });
    expect(screen.getByRole("dialog")).toHaveAccessibleName("重新发送这条消息？");
    expect(screen.getByText(new RegExp(text))).toBeInTheDocument();
    expect(screen.getByRole("dialog").textContent).not.toContain("0 个文件");
    expect(screen.getByRole("dialog").textContent).not.toContain("当前任务会先停止");
  });
  it.each([true, false])("freezes busy copy at send time (busy=%s)", async (busy) => {
    const {intents} = setup(true, busy);
    await waitFor(() => expect(screen.getByTestId("edit-send-button")).not.toBeDisabled());
    fireEvent.click(screen.getByTestId("edit-send-button"));
    await reply(intents().find((i) => i.type === "previewRewind")!.messageId, {
      type: "previewRewindResult", success: true, preview: { revertAvailable: true, revertPaths: ["a.txt"] },
    });
    expect(screen.getByRole("dialog").textContent?.includes("当前任务会先停止")).toBe(busy);
    fireEvent.click(screen.getByTestId("edit-confirm-keep"));
    expect(screen.getByText(busy ? "正在停止当前任务…" : "正在重发…")).toHaveAttribute("role", "status");
  });

  it("keeps two editor instances and their test input events separate",async()=>{
    setup();render(<ComposerSurface {...props}/>);
    await waitFor(()=>screen.getByTestId("composer-input"));
    await act(async()=>window.dispatchEvent(new CustomEvent("tomcat:test:set-composer-value",{detail:{testId:"edit-composer-input",value:"Edited only"}})));
    expect(screen.getByTestId("composer-input").textContent).toBe("");
    expect(screen.getByTestId("edit-composer-input").textContent).toBe("Edited only");
  });
  it.each(["model", "mention", "slash"])("clicking a %s menu option does not cancel editing", async (menu) => {
    const {onClose, intents} = setup(false, false, {
      slashCommands: [{ name: "reload", usage: "/reload", summary: "Reload" }],
    });
    const input = await screen.findByTestId("edit-composer-input");
    if (menu === "model") {
      fireEvent.click(screen.getByTestId("edit-model-select"));
      const option = await screen.findByTestId("edit-model-option");
      fireEvent.pointerDown(option, {button:0});
      fireEvent.click(option);
    } else {
      await act(async () => window.dispatchEvent(new CustomEvent("tomcat:test:set-composer-value", { detail: { testId: "edit-composer-input", value: "" } })));
      await act(async () => fireEvent.paste(input, {clipboardData:{getData:()=>menu === "mention" ? "@app" : "/"}}));
      if (menu === "mention") {
        await waitFor(() => expect(intents().some((i) => i.type === "searchContext")).toBe(true));
        const request = intents().filter((i) => i.type === "searchContext").at(-1)!;
        const data = request.data as {requestId:string};
        await reply(data.requestId, { type: "contextSearchResult", query: "app", truncated:false, matches:[{
          description:"src", reference:{type:"reference",kind:"file",label:"app.ts",path:"src/app.ts",text:null,lineStart:null,lineEnd:null},
        }] });
        const option = await screen.findByTestId("context-search-option");
        fireEvent.pointerDown(option, {button:0});
        fireEvent.mouseDown(option);
        fireEvent.click(option);
      } else {
        const option = await screen.findByTestId("slash-command-option");
        fireEvent.pointerDown(option, {button:0});
        fireEvent.mouseDown(option);
        fireEvent.click(option);
      }
    }
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByTestId("inline-message-editor")).toBeInTheDocument();
  });
  it("Escape closes the menu before cancelling the edit", async () => {
    const {onClose} = setup();
    const input = await screen.findByTestId("edit-composer-input");
    fireEvent.click(screen.getByTestId("edit-model-select"));
    expect(screen.getByTestId("edit-model-dropdown")).toBeInTheDocument();
    fireEvent.keyDown(input, {key:"Escape"});
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.queryByTestId("edit-model-dropdown")).toBeNull();
    fireEvent.keyDown(input, {key:"Escape"});
    expect(onClose).toHaveBeenCalledTimes(1);
  });
  it("pressing an outside scrollbar scrolls instead of cancelling", async () => {
    const {onClose} = setup();
    await screen.findByTestId("edit-composer-input");
    const scroll = document.createElement("div");
    document.body.append(scroll);
    Object.defineProperties(scroll, {scrollHeight:{value:600},clientHeight:{value:100},clientWidth:{value:180}});
    scroll.getBoundingClientRect = () => ({left:0,top:0,right:200,bottom:100,width:200,height:100,x:0,y:0,toJSON:()=>({})});
    fireEvent(scroll, new MouseEvent("pointerdown", {bubbles:true,button:0,clientX:195,clientY:30}));
    expect(onClose).not.toHaveBeenCalled();
    scroll.remove();
  });

  it("cancels on an outside press without a request",async()=>{
    const {intents,onClose}=setup();
    await waitFor(()=>screen.getByTestId("edit-composer-input"));
    await act(async()=>document.body.dispatchEvent(new MouseEvent("pointerdown",{bubbles:true,button:0})));
    expect(onClose).toHaveBeenCalledTimes(1);expect(intents()).toHaveLength(0);
  });
  it("only counts successful native file displays after the selected message",()=>{
    const base:WebviewTimelineItem[]=[message,{id:"bash",type:"tool",toolName:"bash",toolCallId:"b",summary:"wrote",status:"complete",isError:false}];
    expect(messageIdsWithWritesAfter(base).has("old")).toBe(false);
    expect(messageIdsWithWritesAfter([...base,{...base[1],display:{kind:"file",file:"a"}} as WebviewTimelineItem]).has("old")).toBe(true);
  });
});
