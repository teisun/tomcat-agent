/**
 * Must remain byte-for-byte aligned with tomcat's exported Rust transcript constants.
 * These values are protocol markers, not user-facing copy.
 */
export const PENDING_TOOL_RESULT_TEXT = "[pending]";
export const INTERRUPTED_TOOL_RESULT_TEXT = "[interrupted]";
export const UNKNOWN_RESTART_TOOL_RESULT_TEXT =
  "[unknown after restart: this tool may have partially or fully executed; verify its effects before depending on it]";
