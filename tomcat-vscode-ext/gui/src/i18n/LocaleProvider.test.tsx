import { act, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { LocaleProvider, useT } from "./LocaleProvider";
import { setLocale } from "../../../src/shared/i18n";
import { en } from "../../../src/shared/i18n/en";

function Example() { const t = useT(); return <><label>{t("settings.language")}</label><input aria-label="draft" defaultValue="unsent" /></>; }

describe("LocaleProvider", () => {
  afterEach(() => { act(() => setLocale("en")); });
  it("rerenders language without remounting the draft", () => {
    const { rerender, unmount } = render(<LocaleProvider locale="en"><Example /></LocaleProvider>);
    expect(screen.getByText(en["settings.language"])).toBeTruthy();
    const input = screen.getByRole("textbox") as HTMLInputElement;
    input.value = "keep this";
    rerender(<LocaleProvider locale="zh-CN"><Example /></LocaleProvider>);
    expect(screen.getByRole("textbox")).toBe(input);
    expect(input.value).toBe("keep this");
    expect(document.documentElement.lang).toBe("zh-CN");
    unmount();
  });
  it("observes host language changes", () => {
    const { unmount } = render(<LocaleProvider><Example /></LocaleProvider>);
    act(() => setLocale("zh-CN"));
    expect(document.documentElement.lang).toBe("zh-CN");
    act(() => setLocale("en"));
    expect(document.documentElement.lang).toBe("en");
    expect(screen.getByText(en["settings.language"])).toBeTruthy();
    unmount();
  });
});
