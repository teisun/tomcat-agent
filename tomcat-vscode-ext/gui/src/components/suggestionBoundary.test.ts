import { describe, expect, it } from "vitest";
import { Schema } from "@tiptap/pm/model";
import { isTriggerBoundary } from "./suggestionBoundary";

const schema = new Schema({nodes:{doc:{content:"paragraph+"}, paragraph:{content:"inline*", group:"block"}, text:{group:"inline"}, hardBreak:{inline:true, group:"inline"}, reference:{inline:true, group:"inline", atom:true}}});
const text = (value:string) => ({type:"text", text:value});
const paragraph = (content:object[]) => ({type:"paragraph", content});
const boundary = (blocks:object[], from:number) => isTriggerBoundary({state:{doc:schema.nodeFromJSON({type:"doc", content:blocks})}} as never, {from, to:from+1});

describe("one trigger boundary for slash and mention", () => {
  it.each(["/", "@"])("accepts start, spaces, hardBreak and new paragraph for %s", (symbol) => {
    expect(boundary([paragraph([text(symbol)])], 1)).toBe(true);
    expect(boundary([paragraph([text(`help ${symbol}`)])], 6)).toBe(true);
    expect(boundary([paragraph([text("line"), {type:"hardBreak"}, text(symbol)])], 6)).toBe(true);
    expect(boundary([paragraph([text("line")]), paragraph([text(symbol)])], 7)).toBe(true);
  });
  it.each(["src/", "https:/", "https://", "name@", "目录/"])("rejects embedded symbols: %s", (value) => {
    expect(boundary([paragraph([text(value)])], value.length)).toBe(false);
  });
  it("does not mistake a reference chip for empty text", () => {
    expect(boundary([paragraph([{type:"reference"}, text("/")])], 2)).toBe(false);
    expect(boundary([paragraph([{type:"reference"}, text(" /")])], 3)).toBe(true);
  });
});
