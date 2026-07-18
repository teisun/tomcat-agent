export const EXTENSION_ID = "tomcat.tomcat-vscode-ext";

export const TEST_PATH_ENV = "TOMCAT_VSCODE_TEST_PATH";
export const TEST_DEFAULT_CWD_ENV = "TOMCAT_VSCODE_TEST_DEFAULT_CWD";
export const TEST_EXTRA_ARGS_ENV = "TOMCAT_VSCODE_TEST_EXTRA_ARGS";
export const TEST_INFO_ACTION_ENV = "TOMCAT_VSCODE_TEST_INFO_ACTION";
export const TEST_SUPPRESS_EXIT_PROMPT_ENV = "TOMCAT_VSCODE_TEST_SUPPRESS_EXIT_PROMPT";
export const TEST_WARNING_ACTION_ENV = "TOMCAT_VSCODE_TEST_WARNING_ACTION";

export const TOMCAT_CONFIG_SECTION = "tomcat";
export const TOMCAT_EXECUTABLE_NAME = "tomcat";

export const TOMCAT_RESTART_COMMAND = "tomcat.restartServe";
export const TOMCAT_NEW_SESSION_COMMAND = "tomcat.session.new";
export const TOMCAT_LIST_SESSIONS_COMMAND = "tomcat.session.list";
export const TOMCAT_FOCUS_WEBVIEW_COMMAND = "tomcat.ui.focus";
export const TOMCAT_OPEN_SETTINGS_COMMAND = "tomcat.openSettings";
export const TOMCAT_ADD_SELECTION_TO_CHAT_COMMAND = "tomcat.addSelectionToChat";
export const TOMCAT_ADD_FILE_TO_CHAT_COMMAND = "tomcat.addFileToChat";

export const TOMCAT_PLAN_BUILD_COMMAND = "tomcat.plan.build";
export const TOMCAT_PLAN_SELECT_BUILD_MODEL_COMMAND = "tomcat.plan.selectBuildModel";
export const TOMCAT_PLAN_ADD_SELECTION_TO_CHAT_COMMAND = "tomcat.plan.addSelectionToChat";
/** "Preview" (from the native text editor) and "Markdown" (from the custom
 * preview) each open the *other* editor via `vscode.openWith`; there is no
 * in-webview mode toggle, so no display-only ✓ twins are needed. */
export const TOMCAT_PLAN_VIEW_AS_PREVIEW_COMMAND = "tomcat.plan.viewAsPreview";
export const TOMCAT_PLAN_VIEW_AS_MARKDOWN_COMMAND = "tomcat.plan.viewAsMarkdown";

export const TOMCAT_PLAN_CAN_BUILD_CONTEXT_KEY = "tomcat.plan.canBuild";
export const TOMCAT_PLAN_TOOLBAR_STYLE_SETTING = "plan.toolbarStyle";

export const TOMCAT_WEBVIEW_CONTAINER_ID = "tomcat-sidebar";
export const TOMCAT_WEBVIEW_ID = "tomcat.chatView";
