import { describe, expect, it } from "vitest";
import type { WebviewAttachmentView, WebviewSessionSnapshot, WebviewToolCard } from "../types";
import { nextThumbnailTarget } from "./thumbnailBackfill";
const image = (id: string, more: Partial<WebviewAttachmentView> = {}): WebviewAttachmentView => ({
  id,blobSha:id,filename:"photo.png",kind:"image",mimeType:"image/png",fullUri:`full:${id}`,...more,
});
describe("nextThumbnailTarget", () => {
  it("selects pending then tools/messages, one at a time, skipping unavailable/done/attempted", () => {
    const tool: WebviewToolCard={id:"t",toolCallId:"t",toolName:"read",isError:false,status:"complete",type:"tool",attachments:[image("gone",{unavailable:true}),image("done",{hasThumb:true}),image("seen"),image("tool")]};
    const session: Pick<WebviewSessionSnapshot,"timeline"|"pendingAttachments">={pendingAttachments:[{...image("draft"),label:"draft"}],timeline:[tool,{id:"m",type:"message",kind:"user",text:"",attachments:[image("message")]}]};
    const tried=new Set(["seen"]);
    expect(nextThumbnailTarget(session,tried)?.blobSha).toBe("draft"); tried.add("draft");
    expect(nextThumbnailTarget(session,tried)?.blobSha).toBe("tool"); tried.add("tool");
    expect(nextThumbnailTarget(session,tried)?.blobSha).toBe("message"); tried.add("message");
    expect(nextThumbnailTarget(session,tried)).toBeNull();
  });
});
