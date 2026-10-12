import { Node } from "@tiptap/core";
import { NodeViewWrapper, ReactNodeViewRenderer, type NodeViewProps } from "@tiptap/react";
import type { WebviewInstruction } from "../types";
import { InvocationChip } from "./InvocationChip";

export function instructionFromAttrs(attrs: Record<string, unknown>): WebviewInstruction | null {
  if ((attrs.kind !== "command" && attrs.kind !== "skill") || typeof attrs.resourceId !== "string" || typeof attrs.label !== "string") return null;
  return { type:"instruction",kind:attrs.kind,resourceId:attrs.resourceId,label:attrs.label,path:typeof attrs.path === "string" ? attrs.path : undefined,occurrenceId:typeof attrs.occurrenceId === "string" ? attrs.occurrenceId : undefined };
}

function InvocationNodeView({node,selected,editor,getPos}: NodeViewProps) {
  const instruction = instructionFromAttrs(node.attrs);
  if (!instruction) return null;
  return <NodeViewWrapper as="span" className="tc-invocation-node" contentEditable={false} onMouseDown={(event: React.MouseEvent) => {
    event.preventDefault();
    const pos = getPos();
    if (typeof pos === "number") editor.chain().focus().setNodeSelection(pos).run();
  }}><InvocationChip instruction={instruction} selected={selected} /></NodeViewWrapper>;
}

export const InvocationNode = Node.create({
  name:"instruction",group:"inline",inline:true,atom:true,selectable:true,
  addAttributes() {
    return {kind:{default:"command"},resourceId:{default:""},label:{default:""},path:{default:null},occurrenceId:{default:null,parseHTML:()=>crypto.randomUUID()}};
  },
  parseHTML() { return [{tag:"span[data-tomcat-instruction]"}]; },
  renderHTML({HTMLAttributes}) { return ["span",{...HTMLAttributes,"data-tomcat-instruction":"true"},HTMLAttributes.label]; },
  addNodeView() { return ReactNodeViewRenderer(InvocationNodeView); },
});
