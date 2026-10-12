export interface PlanPreviewEditorRegistrationOptions {
  readonly supportsMultipleEditorsPerDocument: false;
  readonly webviewOptions: {
    readonly enableFindWidget: false;
    readonly retainContextWhenHidden: true;
  };
}

/**
 * Registration-time capabilities for the Plan custom editor.
 *
 * Keep this object independent from the VS Code module so its user-visible
 * contract can be unit tested without booting an Extension Host.
 */
export const PLAN_PREVIEW_EDITOR_OPTIONS: PlanPreviewEditorRegistrationOptions =
  Object.freeze({
    supportsMultipleEditorsPerDocument: false,
    webviewOptions: Object.freeze({
      enableFindWidget: false,
      retainContextWhenHidden: true,
    }),
  });
