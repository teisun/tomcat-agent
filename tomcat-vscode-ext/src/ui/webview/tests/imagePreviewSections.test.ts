import { describe, expect, it } from "vitest";
import { imagePreviewSections } from "../imagePreviewSections";
import { WebviewStateStore } from "../state";
import type { WebviewAttachmentView } from "../protocol";
const image=(id:string,more:Partial<WebviewAttachmentView>={}):WebviewAttachmentView=>({id,blobSha:id,kind:"image",mimeType:"image/png",filename:"photo.png",fullUri:`full:${id}`,...more});
describe("imagePreviewSections",()=>{
  it("includes tool images in timeline order, skips unavailable, never substitutes originals for thumbnails",()=>{
    const store=new WebviewStateStore();store.setActiveSession("s1");const session=store.snapshot().sessionViews.s1;
    session.pendingAttachments=[{...image("draft"),label:"draft"}];
    session.timeline=[
      {type:"message",kind:"user",id:"m",text:"",attachments:[image("user"),image("missing",{unavailable:true})]},
      {type:"tool",id:"t",toolCallId:"call",toolName:"read",status:"complete",isError:false,attachments:[image("tool"),image("draft"),image("no-url",{fullUri:null})]},
    ];
    const sections=imagePreviewSections(session);
    expect(sections.map(s=>s.label)).toEqual(["","1","read"]);
    expect(sections.map(s=>s.kind)).toEqual(["pending","sent","tool"]);
    expect(sections.map(s=>s.pictures.map(p=>p.id))).toEqual([["draft"],["user"],["tool"]]);
    expect(sections.flatMap(s=>s.pictures).every(p=>p.thumbUri===null)).toBe(true);
    expect(imagePreviewSections(undefined)).toEqual([]);
  });
});
