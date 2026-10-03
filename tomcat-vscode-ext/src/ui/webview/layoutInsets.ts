import { TOMCAT_CONTROLS_INSET_SETTING, TOMCAT_CONTENT_INSET_SETTING } from "../../constants";

interface ConfigReader { get(key: string): unknown }
function inset(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? Math.max(0,Math.min(40,Math.trunc(value))) : fallback;
}

/** Only finite integers are interpolated into HTML; no live settings channel is needed. */
export function layoutInsetStyle(config: ConfigReader): string {
  return `--tc-controls-inset:${inset(config.get(TOMCAT_CONTROLS_INSET_SETTING),10)}px;--tc-content-inset:${inset(config.get(TOMCAT_CONTENT_INSET_SETTING),15)}px`;
}
