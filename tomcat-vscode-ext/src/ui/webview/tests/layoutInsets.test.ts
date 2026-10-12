import { describe, expect, it } from "vitest";
import { layoutInsetStyle } from "../layoutInsets";

describe("layoutInsetStyle",()=>{
  it.each([
    [undefined,undefined,15,25], [15,25,15,25], [10,15,10,15], [4,24,4,24], [0,0,0,0], [4.9,24.2,4,24], [-1,41,0,40], ["4","24",15,25], [NaN,Infinity,15,25], [40,0,40,0],
  ])("normalizes settings %s / %s",(controls,content,a,b)=>{
    const style=layoutInsetStyle({get:(key)=>key === "layout.controlsInset" ? controls : content});
    expect(style).toBe(`--tc-controls-inset:${a}px;--tc-content-inset:${b}px`);
  });
});
