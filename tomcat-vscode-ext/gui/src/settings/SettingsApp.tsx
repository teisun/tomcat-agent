import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";

import type {
  SettingsHostFrame,
  SettingsIntent,
  SettingsModelCapabilities,
  SettingsModelInput,
  SettingsModelView,
  SettingsProviderKeyView,
  SettingsStateSnapshot,
  VsCodeApiLike,
} from "../../../src/shared/settingsProtocol";
import {
  buildProviderPresets,
  findMatchingProviderPreset,
  type ProviderPreset,
} from "./providerPresets";
import { findReusableModelByName } from "./modelBuiltinMatch";
import {
  deriveRelayFields,
  findConfiguredRelayByBaseUrl,
  RELAY_ID_SEPARATOR,
} from "./relayDerive";
import { KeySlotCombobox, type KeySlotOption } from "./KeySlotCombobox";
import { isValidKeySlotName } from "./keySlot";
import { ConnectorsSettingsView } from "./ConnectorsSettingsView";
import { SettingsShell } from "./SettingsShell";
import { GeneralSettingsView } from "./GeneralSettingsView";
import type { SettingsRoute } from "../../../src/shared/settingsProtocol";

import { isSpeed, type Speed } from "../../../src/shared/modelSpeed";
import { parseCommaList } from "./commaList";
import { useT } from "../i18n/LocaleProvider";
import { type Translator, type MessageKey } from "../../../src/shared/i18n";

type FormState = Omit<SettingsModelInput, "supportedReasoningLevels" | "supportedSpeeds"> & {
  reasoningLevelsText: string | null;
  speedsText: string | null;
};
type FormMode = "create" | "edit";
type DialogKind = "official" | "relay";
const DEFAULT_CONTEXT_WINDOW = 400_000;
const DEFAULT_MAX_OUTPUT_TOKENS = 128_000;
type SettingsDomAction = {
  kind: "clickTestId" | "setInputValue";
  testId?: string;
  value?: string;
};

const RELAY_DEFAULT_CAPABILITIES: SettingsModelCapabilities = {
  files: false,
  reasoning: true,
  tools: true,
  vision: false,
  webSearch: false,
};

const API_OPTIONS = [
  { labelKey: "term.api.openai", value: "openai" },
  { labelKey: "term.api.responses", value: "openai-responses" },
  { labelKey: "term.api.anthropic", value: "anthropic-messages" },
] as const;

const THINKING_FORMAT_OPTIONS = [
  { labelKey: "term.thinking.openai", value: "openai" },
  { labelKey: "term.thinking.deepseek", value: "deepseek" },
  { labelKey: "term.thinking.zai", value: "zai" },
  { labelKey: "term.thinking.doubao", value: "doubao" },
  { labelKey: "term.thinking.anthropic", value: "anthropic" },
  { labelKey: "term.thinking.adaptive", value: "anthropic-adaptive" },
] as const;

const CAPABILITY_OPTIONS = [
  ["vision", "models.capability.vision"],
  ["files", "models.capability.files"],
  ["tools", "models.capability.tools"],
  ["reasoning", "models.capability.reasoning"],
  ["webSearch", "models.capability.webSearch"],
] as const;

function cloneCapabilities(
  capabilities: SettingsModelCapabilities,
): SettingsModelCapabilities {
  return { ...capabilities };
}

function createEmptyForm(): FormState {
  return {
    reasoningLevelsText: null,
    speedsText: null,
    api: "",
    apiKeyEnv: "",
    baseUrl: "",
    capabilities: cloneCapabilities(RELAY_DEFAULT_CAPABILITIES),
    contextWindow: null,
    id: "",
    modelName: "",
    provider: "",
    thinkingFormat: "",
  };
}

function defaultThinkingFormatForApi(api: string): string {
  switch (fieldText(api)) {
    case "openai":
    case "openai-responses":
      return "openai";
    case "anthropic":
    case "anthropic-messages":
      return "anthropic";
    case "deepseek":
      return "deepseek";
    case "zai":
      return "zai";
    case "qwen":
      return "qwen";
    case "doubao":
    case "moonshot":
      return "doubao";
    default:
      return "openai";
  }
}

function fieldText(value: string | null | undefined): string {
  return typeof value === "string" ? value.trim() : "";
}

function maskDraftApiKey(value: string): string {
  const chars = Array.from(value);
  if (chars.length <= 12) {
    return "•".repeat(chars.length);
  }
  return `${chars.slice(0, 8).join("")}${"•".repeat(chars.length - 12)}${chars.slice(-4).join("")}`;
}

function normalizeOptionalText(
  value: string | null | undefined,
): string | null {
  const trimmed = fieldText(value);
  return trimmed ? trimmed : null;
}

function finiteNumberOrNull(value: number | null | undefined): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function hasScheme(value: string): boolean {
  return /^[a-z][a-z0-9+.-]*:\/\//i.test(value);
}

function normalizeBaseUrlInput(
  value: string | null | undefined,
): string | null {
  const trimmed = fieldText(value);
  if (!trimmed) {
    return null;
  }
  return hasScheme(trimmed) ? trimmed : `https://${trimmed}`;
}

function isValidBaseUrl(value: string | null | undefined): boolean {
  const normalized = normalizeBaseUrlInput(value);
  if (!normalized) {
    return false;
  }
  try {
    const parsed = new URL(normalized);
    return Boolean(parsed.hostname);
  } catch {
    return false;
  }
}

function normalizeModel(model: FormState): SettingsModelInput {
  return {
    api: fieldText(model.api),
    apiKeyEnv: normalizeOptionalText(model.apiKeyEnv),
    baseUrl: normalizeBaseUrlInput(model.baseUrl),
    capabilities: cloneCapabilities(model.capabilities),
    contextWindow: finiteNumberOrNull(model.contextWindow),
    contextWindowOptions: Array.isArray(model.contextWindowOptions)
      ? [...model.contextWindowOptions]
      : null,
    description: normalizeOptionalText(model.description),
    id: fieldText(model.id),
    maxOutputTokens: finiteNumberOrNull(model.maxOutputTokens),
    modelName: normalizeOptionalText(model.modelName),
    provider: fieldText(model.provider),
    supportedReasoningLevels: model.reasoningLevelsText === null
      ? null
      : parseCommaList(model.reasoningLevelsText),
    supportedSpeeds: model.speedsText === null
      ? null
      : parseCommaList(model.speedsText).filter(
          (speed): speed is Speed => isSpeed(speed) && speed !== "standard",
        ),
    thinkingFormat: normalizeOptionalText(model.thinkingFormat),
  };
}

function frameIsState(message: unknown): message is SettingsHostFrame {
  return (
    typeof message === "object" &&
    message !== null &&
    (message as { channel?: unknown }).channel === "state"
  );
}

function frameIsTestEvent(message: unknown): message is {
  channel: "event";
  content: {
    action?: SettingsDomAction;
    type: "__test.capture_dom" | "__test.dom_action";
  };
  messageId?: string;
} {
  return (
    typeof message === "object" &&
    message !== null &&
    (message as { channel?: unknown }).channel === "event" &&
    typeof (message as { content?: { type?: unknown } }).content?.type ===
      "string"
  );
}

function readTestIdRect(
  testId: string,
): { height: number; left: number; top: number; width: number } | undefined {
  const el = document.querySelector<HTMLElement>(`[data-testid="${testId}"]`);
  if (!el) {
    return undefined;
  }
  const rect = el.getBoundingClientRect();
  return {
    height: rect.height,
    left: rect.left,
    top: rect.top,
    width: rect.width,
  };
}

function buildSettingsDomSnapshot(): {
  html: string;
  rects: {
    apiKeyInput?: { height: number; left: number; top: number; width: number };
    keySlotBox?: { height: number; left: number; top: number; width: number };
    keySlotInput?: { height: number; left: number; top: number; width: number };
  };
} {
  return {
    html: document.getElementById("root")?.innerHTML ?? "",
    rects: {
      apiKeyInput: readTestIdRect("settings-api-key-input"),
      keySlotBox: readTestIdRect("settings-key-slot-box"),
      keySlotInput: readTestIdRect("settings-key-slot-input"),
    },
  };
}

function runSettingsDomAction(action: SettingsDomAction | undefined): void {
  if (!action || !action.testId) {
    return;
  }
  const target = document.querySelector<HTMLElement>(
    `[data-testid="${action.testId}"]`,
  );
  if (!(target instanceof HTMLElement)) {
    return;
  }
  if (action.kind === "clickTestId") {
    target.click();
    return;
  }
  if (
    action.kind === "setInputValue"
    && (target instanceof HTMLInputElement
      || target instanceof HTMLTextAreaElement
      || target instanceof HTMLSelectElement)
  ) {
    const descriptor = Object.getOwnPropertyDescriptor(
      Object.getPrototypeOf(target),
      "value",
    );
    if (descriptor?.set) {
      descriptor.set.call(target, action.value ?? "");
    } else {
      target.value = action.value ?? "";
    }
    target.dispatchEvent(new Event("input", { bubbles: true }));
    target.dispatchEvent(new Event("change", { bubbles: true }));
  }
}

function modelToForm(model: SettingsModelView): FormState {
  return {
    api: model.api,
    apiKeyEnv: model.apiKeyEnv,
    baseUrl: model.baseUrl ?? "",
    capabilities: cloneCapabilities(model.capabilities),
    contextWindow: model.contextWindow ?? null,
    contextWindowOptions: model.contextWindowOptions ?? null,
    description: model.description ?? null,
    id: model.id,
    maxOutputTokens: model.maxOutputTokens ?? null,
    modelName: model.modelName ?? "",
    provider: model.provider,
    reasoningLevelsText: model.supportedReasoningLevels?.join(", ") ?? null,
    speedsText: model.supportedSpeeds?.join(", ") ?? null,
    thinkingFormat: model.thinkingFormat ?? "",
  };
}

function formatApiLabel(api: string, t: Translator): string {
  const entry = API_OPTIONS.find(entry => entry.value === api);
  return entry ? t(entry.labelKey) : api;
}

function formatThinkingLabel(thinkingFormat: string | null | undefined, t: Translator): string {
  const entry = THINKING_FORMAT_OPTIONS.find(entry => entry.value === thinkingFormat);
  return entry ? t(entry.labelKey) : thinkingFormat ?? "";
}

function formatVersionLabel(version: string | null | undefined): string {
  const trimmed = fieldText(version);
  return trimmed ? `v${trimmed}` : "vunknown";
}

function buildServeVersionWarning(state: SettingsStateSnapshot, t: Translator): string | null {
  const expected = fieldText(state.expectedCliVersion);
  const server = fieldText(state.serverVersion);
  if (!server) {
    return t("models.versionMissing");
  }
  if (expected && server !== expected) {
    return t("models.versionMismatch", { expected, server });
  }
  return null;
}

function modelKeyEnvName(model: SettingsModelView): string {
  return fieldText(model.apiKeyEnv);
}

function fallbackModelName(model: SettingsModelView): string {
  const explicit = fieldText(model.modelName);
  if (explicit) {
    return explicit;
  }
  const fromRelay = model.id.split(RELAY_ID_SEPARATOR).pop();
  return fromRelay?.trim() || model.id;
}

function buildModelDetails(model: SettingsModelView, t: Translator): Array<{ id: string; label: string; value: string }> {
  return [
    { id: "source", label: t("models.source"), value: t(model.source === "user" ? "models.user" : "models.builtin") },
    { id: "api", label: t("term.api"), value: formatApiLabel(model.api, t) },
    { id: "provider", label: t("models.provider"), value: model.provider },
    { id: "key", label: t("models.apiKeyEnv"), value: modelKeyEnvName(model) },
    { id: "url", label: t("term.baseUrl"), value: model.baseUrl ?? "" },
    { id: "thinking", label: t("thinking.title"), value: formatThinkingLabel(model.thinkingFormat, t) },
    { id: "context", label: t("models.contextWindow"), value: typeof model.contextWindow === "number" ? String(model.contextWindow) : "" },
    { id: "name", label: t("models.modelName"), value: model.modelName ?? "" },
  ].filter(entry => entry.value.trim().length > 0);
}

function buildConfiguredKeyLabel(entry: SettingsProviderKeyView, t: Translator): string {
  return entry.keyPresent ? t("models.keyConfigured", { name: entry.envName }) : entry.envName;
}

function buildKeySlotOptions(
  suggestedEnvName: string,
  providerKeys: SettingsProviderKeyView[],
  t: Translator,
): KeySlotOption[] {
  const options: KeySlotOption[] = [];
  const seen = new Set<string>();
  if (suggestedEnvName) {
    const existing = providerKeys.find(
      (entry) => entry.envName === suggestedEnvName,
    );
    options.push({
      envName: suggestedEnvName,
      group: "suggested",
      keyPresent: existing?.keyPresent ?? false,
      label: t(existing?.keyPresent ? "models.keyConfigured" : "models.keySuggested", { name: suggestedEnvName }),
    });
    seen.add(suggestedEnvName);
  }
  for (const entry of providerKeys) {
    if (seen.has(entry.envName)) {
      continue;
    }
    options.push({
      envName: entry.envName,
      group: "saved",
      keyPresent: entry.keyPresent,
      label: buildConfiguredKeyLabel(entry, t),
    });
    seen.add(entry.envName);
  }
  return options;
}

function defaultDialogKindForPresets(presets: ProviderPreset[]): DialogKind {
  return presets.length > 0 ? "official" : "relay";
}

function tabIdForDialogKind(kind: DialogKind): string {
  return `settings-model-tab-${kind}`;
}

function panelIdForDialogKind(kind: DialogKind): string {
  return `settings-model-panel-${kind}`;
}

function focusDialogTab(kind: DialogKind): void {
  window.requestAnimationFrame(() => {
    const element = window.document.getElementById(tabIdForDialogKind(kind));
    if (element instanceof HTMLButtonElement) {
      element.focus();
    }
  });
}

function inferDialogKind(
  model: SettingsModelView,
  presets: ProviderPreset[],
): {
  dialogKind: DialogKind;
  selectedProvider: string;
} {
  const matchingPreset = findMatchingProviderPreset(model, presets);
  if (matchingPreset) {
    return {
      dialogKind: "official",
      selectedProvider: matchingPreset.provider,
    };
  }

  return {
    dialogKind: "relay",
    selectedProvider: presets[0]?.provider ?? "",
  };
}

function buildEditForm(
  model: SettingsModelView,
  dialogKind: DialogKind,
  preset: ProviderPreset | null,
): FormState {
  const form = modelToForm({
    ...model,
    modelName: fallbackModelName(model),
  });

  if (dialogKind === "official" && preset) {
    if (form.api === preset.api) {
      form.api = "";
    }
    if (fieldText(form.baseUrl) === preset.baseUrl) {
      form.baseUrl = "";
    }
    if (fieldText(form.apiKeyEnv) === preset.apiKeyEnv) {
      form.apiKeyEnv = "";
    }
    if (fieldText(form.thinkingFormat) === preset.thinkingFormat) {
      form.thinkingFormat = "";
    }
    form.provider = "";
    return form;
  }

  const relayDerived = deriveRelayFields(
    form.baseUrl ?? "",
    form.modelName ?? "",
    form.api,
    RELAY_ID_SEPARATOR,
  );
  if (fieldText(form.provider) === relayDerived.provider) {
    form.provider = "";
  }
  if (fieldText(form.apiKeyEnv) === relayDerived.apiKeyEnv) {
    form.apiKeyEnv = "";
  }
  return form;
}

type SettingsIntentPayload = SettingsIntent extends infer Intent
  ? Intent extends SettingsIntent
    ? Omit<Intent, "messageId">
    : never
  : never;

function send(
  vscodeApi: VsCodeApiLike<SettingsIntent>,
  message: SettingsIntentPayload,
): void {
  vscodeApi.postMessage({
    ...message,
    messageId: `${message.type}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
  } as SettingsIntent);
}

export function SettingsApp({
  vscodeApi,
  initialRoute = "models",
}: {
  vscodeApi: VsCodeApiLike<SettingsIntent>;
  initialRoute?: SettingsRoute;
}) {
  const t = useT();
  const [state, setState] = useState<SettingsStateSnapshot>({
    capabilities: {
      listModels: false,
      listProviderKeys: false,
      removeModel: false,
      setProviderKey: false,
      upsertModel: false,
    },
    models: [],
    providerKeys: [],
    ready: false,
    route: initialRoute,  });
  const [dialogKind, setDialogKind] = useState<DialogKind>("official");
  const [form, setForm] = useState<FormState>(() => createEmptyForm());
  const [draftApiKey, setDraftApiKey] = useState("");
  const [isApiKeyFocused, setIsApiKeyFocused] = useState(false);
  const [inlineApiKeys, setInlineApiKeys] = useState<Record<string, string>>(
    {},
  );
  const [selectedModelId, setSelectedModelId] = useState<string | null>(null);
  const [selectedProvider, setSelectedProvider] = useState("");
  const [formMode, setFormMode] = useState<FormMode>("create");
  // `true` means the user has taken ownership of the suggestion, including by clearing it.
  const [idManuallyEdited, setIdManuallyEdited] = useState(false);
  const [isFormOpen, setIsFormOpen] = useState(false);
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [validationError, setValidationError] = useState<{ key: MessageKey; args?: Record<string, string | number> } | null>(null);
  const [isKeySlotRefreshing, setIsKeySlotRefreshing] = useState(false);
  const [keySlotRefreshFeedback, setKeySlotRefreshFeedback] = useState<
    MessageKey | null
  >(null);
  const [replacementConfirmation, setReplacementConfirmation] = useState<{
    envName: string;
    modelIds: string[];
  } | null>(null);
  const [deleteConfirmation, setDeleteConfirmation] = useState<string | null>(null);
  const [deletingModelId, setDeletingModelId] = useState<string | null>(null);
  const keySlotRefreshPendingRef = useRef(false);

  useEffect(() => {
    const handleMessage = (event: MessageEvent<unknown>) => {
      if (frameIsTestEvent(event.data)) {
        if (event.data.content.type === "__test.capture_dom") {
          (vscodeApi as VsCodeApiLike<unknown>).postMessage({
            data: buildSettingsDomSnapshot(),
            messageId: event.data.messageId ?? `settings-dom-${Date.now()}`,
            type: "__test.dom_snapshot",
          });
          return;
        }
        if (event.data.content.type === "__test.dom_action") {
          runSettingsDomAction(event.data.content.action);
          return;
        }
      }
      if (!frameIsState(event.data)) {
        return;
      }
      if (keySlotRefreshPendingRef.current) {
        keySlotRefreshPendingRef.current = false;
        setIsKeySlotRefreshing(false);
        setKeySlotRefreshFeedback(
          event.data.content.error ? null : "models.keysRefreshed",
        );
      }
      setState(event.data.content);
    };
    window.addEventListener("message", handleMessage);
    send(vscodeApi, {
      data: {
        route: initialRoute,      },
      type: "settings.ready",
    });
    return () => {
      window.removeEventListener("message", handleMessage);
    };
  }, [vscodeApi, initialRoute]);

  useEffect(() => {
    if (!keySlotRefreshFeedback) {
      return;
    }
    const timer = window.setTimeout(() => {
      setKeySlotRefreshFeedback(null);
    }, 2500);
    return () => {
      window.clearTimeout(timer);
    };
  }, [keySlotRefreshFeedback]);

  useEffect(() => {
    const receipt = state.modelRemovalReceipt;
    if (!receipt || receipt.modelId !== deletingModelId) {
      return;
    }
    setDeletingModelId(null);
    if (receipt.success) {
      setIsFormOpen(false);
      setFormMode("create");
      setDeleteConfirmation(null);
    }
  }, [deletingModelId, state.modelRemovalReceipt]);

  const providerPresets = useMemo(
    () => buildProviderPresets(state.models),
    [state.models],
  );
  const providerPresetByProvider = useMemo(
    () => new Map(providerPresets.map((preset) => [preset.provider, preset])),
    [providerPresets],
  );
  const providerKeysByEnv = useMemo(
    () => new Map(state.providerKeys.map((entry) => [entry.envName, entry])),
    [state.providerKeys],
  );

  useEffect(() => {
    if (
      !isFormOpen ||
      dialogKind !== "official" ||
      selectedProvider ||
      providerPresets.length === 0
    ) {
      return;
    }
    const firstPreset = providerPresets[0];
    setSelectedProvider(firstPreset.provider);
    setForm((current) => ({
      ...current,
      capabilities: cloneCapabilities(firstPreset.capabilities),
    }));
  }, [dialogKind, isFormOpen, providerPresets, selectedProvider]);

  useEffect(() => {
    if (!isFormOpen) {
      return;
    }
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        closeForm();
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
    };
  }, [isFormOpen]);

  const readyModels = useMemo(
    () => state.models.filter((model) => model.keyPresent),
    [state.models],
  );
  const needsKeyModels = useMemo(
    () => state.models.filter((model) => !model.keyPresent),
    [state.models],
  );
  const selectedModel = useMemo(
    () => state.models.find((model) => model.id === selectedModelId) ?? null,
    [selectedModelId, state.models],
  );

  const selectedPreset =
    dialogKind === "official"
      ? (providerPresetByProvider.get(selectedProvider) ??
        providerPresets[0] ??
        null)
      : null;

  const effectiveModelName = fieldText(form.modelName);
  const reusableModelByName = useMemo(
    () => findReusableModelByName(state.models, effectiveModelName),
    [effectiveModelName, state.models],
  );
  // A relay id belongs to the gateway, while modelName identifies the upstream model.
  // Reuse is a creation-time snapshot: a user's explicit models.toml entry
  // wins over the embedded catalog, but neither changes an existing relay.
  const automaticReusableModel =
    formMode === "create" && dialogKind === "relay" ? reusableModelByName : null;
  const endpointReuse =
    formMode === "create" && dialogKind === "relay"
      ? findConfiguredRelayByBaseUrl(state.models, form.baseUrl ?? "")
      : null;
  const relayAutofillSource = automaticReusableModel
    ? `${automaticReusableModel.source}:${automaticReusableModel.id}`
    : null;
  const lastRelayAutofillSource = useRef<string | null>(null);

  // A reuse match is a creation-time snapshot, not a permanent overlay.  Put
  // its editable values into the draft once so every ordinary form control can
  // subsequently override them.  Context and output limits remain calculated
  // catalog values because those controls are intentionally read-only here.
  useEffect(() => {
    if (!isFormOpen || !relayAutofillSource || !automaticReusableModel) {
      if (!isFormOpen) {
        lastRelayAutofillSource.current = null;
      }
      return;
    }
    if (lastRelayAutofillSource.current === relayAutofillSource) {
      return;
    }
    lastRelayAutofillSource.current = relayAutofillSource;
    setForm((current) => ({
      ...current,
      api: automaticReusableModel.api,
      capabilities: cloneCapabilities(automaticReusableModel.capabilities),
      description: automaticReusableModel.description ?? null,
      reasoningLevelsText: automaticReusableModel.supportedReasoningLevels?.join(", ") ?? null,
      speedsText: automaticReusableModel.supportedSpeeds?.join(", ") ?? null,
      thinkingFormat: automaticReusableModel.thinkingFormat ?? "",
    }));
  }, [
    automaticReusableModel,
    isFormOpen,
    relayAutofillSource,
  ]);
  const effectiveApi =
    dialogKind === "official"
      ? fieldText(form.api) || selectedPreset?.api || ""
      : fieldText(form.api) || automaticReusableModel?.api || "openai";
  const effectiveCapabilities = cloneCapabilities(
    form.capabilities,
  );
  // The current Anthropic Messages adapter has no files attachment transport.
  // Keep inherited relay capabilities valid for the backend's mutable-model check.
  if (effectiveApi === "anthropic-messages") {
    effectiveCapabilities.files = false;
  }
  const relayDerived = useMemo(
    () =>
      deriveRelayFields(
        form.baseUrl ?? "",
        form.modelName ?? "",
        effectiveApi,
        RELAY_ID_SEPARATOR,
      ),
    [effectiveApi, form.baseUrl, form.modelName],
  );
  const effectiveProvider =
    dialogKind === "official"
      ? fieldText(form.provider) || selectedPreset?.provider || ""
      : fieldText(form.provider) || endpointReuse?.provider || relayDerived.provider;
  const effectiveBaseUrl =
    dialogKind === "official"
      ? (normalizeBaseUrlInput(form.baseUrl) ?? selectedPreset?.baseUrl ?? null)
      : normalizeBaseUrlInput(form.baseUrl);
  const suggestedApiKeyEnv =
    dialogKind === "official"
      ? (selectedPreset?.apiKeyEnv ?? "")
      : endpointReuse?.apiKeyEnv ?? relayDerived.apiKeyEnv;
  const effectiveApiKeyEnv = fieldText(form.apiKeyEnv) || suggestedApiKeyEnv;
  const derivedId =
    dialogKind === "official" ? effectiveModelName : relayDerived.id;
  const effectiveId =
    formMode === "edit"
      ? (selectedModelId ?? fieldText(form.id))
      : idManuallyEdited
        ? fieldText(form.id)
        : derivedId;
  const normalizedContextWindow = finiteNumberOrNull(form.contextWindow);
  const automaticContextWindow = finiteNumberOrNull(
    automaticReusableModel?.contextWindow,
  );
  const effectiveContextWindow =
    formMode === "create"
      ? (automaticContextWindow ?? normalizedContextWindow ?? DEFAULT_CONTEXT_WINDOW)
      : (normalizedContextWindow ?? automaticContextWindow ?? DEFAULT_CONTEXT_WINDOW);
  const normalizedMaxOutputTokens = finiteNumberOrNull(form.maxOutputTokens);
  const automaticMaxOutputTokens = finiteNumberOrNull(
    automaticReusableModel?.maxOutputTokens,
  );
  const effectiveMaxOutputTokens =
    normalizedMaxOutputTokens ?? automaticMaxOutputTokens ?? DEFAULT_MAX_OUTPUT_TOKENS;
  const effectiveContextWindowOptions =
    formMode === "create"
      ? (automaticReusableModel?.contextWindowOptions ??
        form.contextWindowOptions ??
        null)
      : (form.contextWindowOptions ??
        automaticReusableModel?.contextWindowOptions ??
        null);
  const effectiveThinkingFormat =
    fieldText(form.thinkingFormat) ||
    (dialogKind === "official"
      ? (selectedPreset?.thinkingFormat ??
        defaultThinkingFormatForApi(effectiveApi))
      : (automaticReusableModel?.thinkingFormat ??
        defaultThinkingFormatForApi(effectiveApi)));
  const effectiveModel: SettingsModelInput = normalizeModel({
    ...form,
    api: effectiveApi,
    apiKeyEnv: effectiveApiKeyEnv,
    baseUrl: effectiveBaseUrl ?? "",
    capabilities: effectiveCapabilities,
    contextWindow: effectiveContextWindow,
    contextWindowOptions: effectiveContextWindowOptions,
    id: effectiveId,
    maxOutputTokens: effectiveMaxOutputTokens,
    modelName: effectiveModelName,
    provider: effectiveProvider,
    description: form.description,
    reasoningLevelsText: form.reasoningLevelsText,
    speedsText: form.speedsText,
    thinkingFormat: effectiveThinkingFormat,
  });

  const keySlotOptions = useMemo(
    () => buildKeySlotOptions(suggestedApiKeyEnv, state.providerKeys, t),
    [state.providerKeys, suggestedApiKeyEnv, t],
  );
  const selectedKeyStatus = effectiveApiKeyEnv
    ? (providerKeysByEnv.get(effectiveApiKeyEnv) ?? null)
    : null;
  const effectiveKeyPresent = Boolean(selectedKeyStatus?.keyPresent);
  const builtinCollision = useMemo(
    () =>
      state.models.find(
        (model) => model.source === "builtin" && model.id === effectiveId,
      ) ?? null,
    [effectiveId, state.models],
  );
  const officialPresetUnavailable =
    dialogKind === "official" && selectedPreset === null;
  const showSharedFormFields =
    dialogKind === "relay" || selectedPreset !== null;

  function resetForm() {
    const nextDialogKind = defaultDialogKindForPresets(providerPresets);
    setDialogKind(nextDialogKind);
    setDraftApiKey("");
    setIsApiKeyFocused(false);
    setIsKeySlotRefreshing(false);
    setKeySlotRefreshFeedback(null);
    setSelectedModelId(null);
    setIdManuallyEdited(false);
    setSelectedProvider(providerPresets[0]?.provider ?? "");
    setShowAdvanced(false);
    setValidationError(null);
    setReplacementConfirmation(null);
    keySlotRefreshPendingRef.current = false;
    setForm({
      ...createEmptyForm(),
      capabilities: cloneCapabilities(
        providerPresets[0]?.capabilities ?? RELAY_DEFAULT_CAPABILITIES,
      ),
    });
  }

  function closeForm() {
    if (deletingModelId) {
      return;
    }
    setIsFormOpen(false);
    setFormMode("create");
    resetForm();
  }

  function openCreateForm() {
    resetForm();
    setFormMode("create");
    setIsFormOpen(true);
  }

  function openEditForm(model: SettingsModelView) {
    const inferred = inferDialogKind(model, providerPresets);
    const preset =
      inferred.dialogKind === "official"
        ? (providerPresetByProvider.get(inferred.selectedProvider) ?? null)
        : null;
    setSelectedModelId(model.id);
    setIdManuallyEdited(false);
    setDraftApiKey("");
    setIsApiKeyFocused(false);
    setValidationError(null);
    setDialogKind(inferred.dialogKind);
    setSelectedProvider(inferred.selectedProvider);
    setShowAdvanced(false);
    setForm(buildEditForm(model, inferred.dialogKind, preset));
    setFormMode("edit");
    setIsFormOpen(true);
  }

  function handleDialogKindChange(nextKind: DialogKind) {
    if (nextKind === dialogKind) {
      return;
    }
    setDialogKind(nextKind);
    setShowAdvanced(false);
    setValidationError(null);
    if (nextKind === "official") {
      const nextProvider =
        selectedProvider || providerPresets[0]?.provider || "";
      const preset =
        providerPresetByProvider.get(nextProvider) ??
        providerPresets[0] ??
        null;
      setSelectedProvider(nextProvider);
      setForm((current) => ({
        ...createEmptyForm(),
        capabilities: cloneCapabilities(
          preset?.capabilities ?? RELAY_DEFAULT_CAPABILITIES,
        ),
        contextWindow: current.contextWindow ?? null,
        id: formMode === "edit" ? (selectedModelId ?? current.id) : current.id,
        modelName: current.modelName ?? "",
      }));
      return;
    }

    setForm((current) => ({
      ...createEmptyForm(),
      // Leave API unset for a fresh relay so a matching upstream model can
      // supply its transport (for example Anthropic Messages for Claude).
      api: fieldText(current.api),
      baseUrl: current.baseUrl ?? "",
      capabilities: cloneCapabilities(RELAY_DEFAULT_CAPABILITIES),
      contextWindow: current.contextWindow ?? null,
      id: formMode === "edit" ? (selectedModelId ?? current.id) : current.id,
      modelName: current.modelName ?? "",
    }));
  }

  function handleDialogTabKeyDown(
    currentKind: DialogKind,
    event: ReactKeyboardEvent<HTMLButtonElement>,
  ) {
    let nextKind: DialogKind | null = null;
    switch (event.key) {
      case "ArrowLeft":
      case "ArrowUp":
        nextKind = currentKind === "official" ? "relay" : "official";
        break;
      case "ArrowRight":
      case "ArrowDown":
        nextKind = currentKind === "official" ? "relay" : "official";
        break;
      case "Home":
        nextKind = "official";
        break;
      case "End":
        nextKind = "relay";
        break;
      default:
        return;
    }

    event.preventDefault();
    handleDialogKindChange(nextKind);
    focusDialogTab(nextKind);
  }

  function handlePresetChange(nextProvider: string) {
    const preset = providerPresetByProvider.get(nextProvider) ?? null;
    setSelectedProvider(nextProvider);
    setValidationError(null);
    setForm((current) => ({
      ...current,
      api: "",
      apiKeyEnv: "",
      baseUrl: "",
      capabilities: cloneCapabilities(
        preset?.capabilities ?? RELAY_DEFAULT_CAPABILITIES,
      ),
      provider: "",
      thinkingFormat: "",
    }));
  }

  function handleApiChange(nextApi: string) {
    setForm((current) => {
      const currentApi =
        dialogKind === "official"
          ? fieldText(current.api) || selectedPreset?.api || ""
          : fieldText(current.api) || "openai";
      const currentDefaultThinkingFormat =
        defaultThinkingFormatForApi(currentApi);
      const currentThinkingFormat = fieldText(current.thinkingFormat);
      const nextThinkingFormat =
        !currentThinkingFormat ||
        currentThinkingFormat === currentDefaultThinkingFormat
          ? defaultThinkingFormatForApi(nextApi)
          : current.thinkingFormat;
      return {
        ...current,
        api:
          dialogKind === "official" &&
          selectedPreset &&
          nextApi === selectedPreset.api
            ? ""
            : nextApi,
        thinkingFormat: nextThinkingFormat,
      };
    });
  }

  function handleKeySlotChange(nextEnvName: string) {
    setForm((current) => ({
      ...current,
      apiKeyEnv: nextEnvName === suggestedApiKeyEnv ? "" : nextEnvName,
    }));
  }

  function handleCapabilityChange(
    key: keyof SettingsModelCapabilities,
    checked: boolean,
  ) {
    setForm((current) => ({
      ...current,
      capabilities: {
        ...current.capabilities,
        [key]: checked,
      },
    }));
  }

  function handleKeySlotRefresh() {
    if (!state.capabilities.listProviderKeys || isKeySlotRefreshing) {
      return;
    }
    keySlotRefreshPendingRef.current = true;
    setIsKeySlotRefreshing(true);
    setKeySlotRefreshFeedback(null);
    send(vscodeApi, { type: "listProviderKeys" });
  }

  function submitModelSave() {
    send(vscodeApi, {
      data: {
        model: effectiveModel,
        providerKey:
          draftApiKey.trim() && effectiveApiKeyEnv
            ? {
                envName: effectiveApiKeyEnv,
                value: draftApiKey.trim(),
              }
            : undefined,
      },
      type: "upsertModel",
    });
    closeForm();
  }

  function handleSave() {
    if (parseCommaList(form.speedsText ?? "").some((speed) => !isSpeed(speed))) {
      setValidationError({ key: "models.validate.speeds" });
      return;
    }
    if (dialogKind === "official" && !selectedPreset) {
      setValidationError({ key: "models.validate.preset" });
      return;
    }
    if (!effectiveModelName) {
      setValidationError({ key: "models.validate.name" });
      return;
    }
    if (dialogKind === "relay" && !fieldText(form.baseUrl)) {
      setValidationError({ key: "models.validate.baseRequired" });
      return;
    }
    if (dialogKind === "relay" && !isValidBaseUrl(form.baseUrl)) {
      setValidationError({ key: "models.validate.baseInvalid" });
      return;
    }
    if (!effectiveModel.id || !effectiveModel.provider || !effectiveModel.api) {
      setValidationError({ key: "models.validate.identity" });
      return;
    }
    if (!effectiveApiKeyEnv) {
      setValidationError({ key: "models.validate.slotRequired" });
      return;
    }
    if (!isValidKeySlotName(effectiveApiKeyEnv)) {
      setValidationError({ key: "models.validate.slotInvalid" });
      return;
    }
    if (!effectiveKeyPresent && !draftApiKey.trim()) {
      setValidationError({ key: "models.validate.keyRequired", args: { name: effectiveApiKeyEnv } });
      return;
    }
    if (draftApiKey.trim() && !state.capabilities.setProviderKey) {
      setValidationError({ key: "models.validate.keyUnsupported" });
      return;
    }
    setValidationError(null);
    const affectedModelIds = state.models
      .filter(
        (model) =>
          model.apiKeyEnv === effectiveApiKeyEnv &&
          model.id !== selectedModel?.id,
      )
      .map((model) => model.id);
    if (
      effectiveKeyPresent &&
      draftApiKey.trim() &&
      affectedModelIds.length > 0
    ) {
      setReplacementConfirmation({
        envName: effectiveApiKeyEnv,
        modelIds: affectedModelIds,
      });
      return;
    }
    submitModelSave();
  }

  function handleDelete() {
    if (!selectedModel || selectedModel.source !== "user" || deletingModelId) {
      return;
    }
    setDeleteConfirmation(selectedModel.id);
  }

  function confirmDelete() {
    if (!deleteConfirmation || deletingModelId) {
      return;
    }
    setDeletingModelId(deleteConfirmation);
    setDeleteConfirmation(null);
    send(vscodeApi, {
      data: { modelId: deleteConfirmation },
      type: "removeModel",
    });
  }

  function handleInlineSave(model: SettingsModelView) {
    const value = inlineApiKeys[model.id]?.trim() ?? "";
    if (!value) {
      return;
    }
    send(vscodeApi, {
      data: {
        envName: model.apiKeyEnv,
        value,
      },
      type: "setProviderKey",
    });
    setInlineApiKeys((current) => ({
      ...current,
      [model.id]: "",
    }));
  }

  const formTitle =
    formMode === "edit" && selectedModelId
      ? t("models.editTitle", { name: selectedModelId })
      : t("models.addTitle");
  const formDescription =
    formMode === "edit"
      ? t("models.editDescription")
      : t("models.addDescription");

  const saveDisabledReason = !state.capabilities.upsertModel
    ? t("models.disable.write")
    : officialPresetUnavailable
      ? t("models.disable.preset")
      : !effectiveModelName
        ? t("models.disable.name")
        : !effectiveApiKeyEnv
          ? t("models.validate.slotRequired")
          : !isValidKeySlotName(effectiveApiKeyEnv)
            ? t("models.disable.slotInvalid")
            : dialogKind === "relay" && !fieldText(form.baseUrl)
              ? t("models.disable.baseRequired")
              : !effectiveKeyPresent && !draftApiKey.trim()
                ? t("models.disable.keyRequired", { name: effectiveApiKeyEnv })
                : null;
  const saveDisabled = saveDisabledReason !== null;
  const serveVersionWarning = buildServeVersionWarning(state, t);

  if (state.route === "general") {
    return <GeneralSettingsView state={state} vscodeApi={vscodeApi} />;
  }

  if (state.route === "connectors") {
    return <ConnectorsSettingsView state={state} vscodeApi={vscodeApi} />;
  }

  return (
    <SettingsShell state={state} vscodeApi={vscodeApi}>
      <main className="tc-settings-shell__content">
        <header className="tc-settings-shell__header">
          <div>
            <h1>{t("models.title")}</h1>
            <p>{t("models.description")}</p>
          </div>
          <button
            className="tc-button tc-button--secondary"
            data-testid="settings-add-model"
            disabled={!state.capabilities.upsertModel}
            onClick={openCreateForm}
            type="button"
          >
            {t("models.add")}
          </button>
        </header>

        {serveVersionWarning ? (
          <div className="tc-banner tc-banner--warning">
            {serveVersionWarning}
          </div>
        ) : null}
        {state.error ? (
          <div className="tc-banner tc-banner--warning">{state.error}</div>
        ) : null}
        {state.warnings?.map((warning) => (
          <div key={warning} className="tc-banner tc-banner--warning">
            {warning}
          </div>
        ))}
        {state.status ? <div className="tc-banner">{state.status}</div> : null}

        {!state.capabilities.listModels ? (
          <section className="tc-empty-state">
            <h2>{t("models.unavailable")}</h2>
            <p>{t("models.unavailableHint")}</p>
          </section>
        ) : (
          <div className="tc-settings-groups">
            <section className="tc-settings-group">
              <h2 className="tc-settings-group__title">{t("models.ready")}</h2>
              {readyModels.length === 0 ? (
                <div className="tc-session-dropdown__empty">
                  {t("models.noneReady")}
                </div>
              ) : (
                readyModels.map((model) => {
                  const details = buildModelDetails(model, t);
                  return (
                    <article className="tc-settings-model" key={model.id}>
                      <div className="tc-settings-model__header">
                        <div className="tc-settings-model__identity">
                          <span
                            aria-label={t("models.ready")}
                            className="tc-settings-model__status-dot tc-settings-model__status-dot--ready"
                            role="img"
                          />
                          <strong>{model.id}</strong>
                        </div>
                        <div className="tc-settings-model__actions">
                          <div className="tc-settings-model__tooltip-anchor">
                            <button
                              aria-label={t("models.details", { name: model.id })}
                              className="tc-settings-model__info"
                              type="button"
                            >
                              <span
                                aria-hidden="true"
                                className="codicon codicon-info"
                              />
                            </button>
                            <div
                              className="tc-settings-model__tooltip"
                              role="tooltip"
                            >
                              <dl className="tc-settings-model__tooltip-list">
                                {details.map((entry) => (
                                  <div
                                    className="tc-settings-model__tooltip-row"
                                    key={`${model.id}-${entry.id}`}
                                  >
                                    <dt>{entry.label}</dt>
                                    <dd>{entry.value}</dd>
                                  </div>
                                ))}
                              </dl>
                            </div>
                          </div>
                          <button
                            className="tc-button tc-button--secondary"
                            data-testid={`settings-edit-${model.id}`}
                            onClick={() => openEditForm(model)}
                            type="button"
                          >
                            {t("models.edit")}
                          </button>
                        </div>
                      </div>
                    </article>
                  );
                })
              )}
            </section>

            <section className="tc-settings-group">
              <h2 className="tc-settings-group__title">{t("models.needsKey")}</h2>
              {needsKeyModels.length === 0 ? (
                <div className="tc-session-dropdown__empty">
                  {t("models.noMissingKey")}
                </div>
              ) : (
                needsKeyModels.map((model) => {
                  const details = buildModelDetails(model, t);
                  return (
                    <article className="tc-settings-model" key={model.id}>
                      <div className="tc-settings-model__header">
                        <div className="tc-settings-model__identity">
                          <span
                            aria-label={t("models.needsKey")}
                            className="tc-settings-model__status-dot tc-settings-model__status-dot--missing"
                            role="img"
                          />
                          <strong>{model.id}</strong>
                        </div>
                        <div className="tc-settings-model__actions">
                          <div className="tc-settings-model__tooltip-anchor">
                            <button
                              aria-label={t("models.details", { name: model.id })}
                              className="tc-settings-model__info"
                              type="button"
                            >
                              <span
                                aria-hidden="true"
                                className="codicon codicon-info"
                              />
                            </button>
                            <div
                              className="tc-settings-model__tooltip"
                              role="tooltip"
                            >
                              <dl className="tc-settings-model__tooltip-list">
                                {details.map((entry) => (
                                  <div
                                    className="tc-settings-model__tooltip-row"
                                    key={`${model.id}-${entry.id}`}
                                  >
                                    <dt>{entry.label}</dt>
                                    <dd>{entry.value}</dd>
                                  </div>
                                ))}
                              </dl>
                            </div>
                          </div>
                          <button
                            className="tc-button tc-button--secondary"
                            data-testid={`settings-edit-${model.id}`}
                            onClick={() => openEditForm(model)}
                            type="button"
                          >
                            {t("models.edit")}
                          </button>
                        </div>
                      </div>
                      <div className="tc-settings-inline-key">
                        <input
                          autoComplete="off"
                          className="tc-input"
                          onChange={(event) =>
                            setInlineApiKeys((current) => ({
                              ...current,
                              [model.id]: event.target.value,
                            }))
                          }
                          placeholder={t("models.saveKey", { name: modelKeyEnvName(model) })}
                          type="password"
                          value={inlineApiKeys[model.id] ?? ""}
                        />
                        <button
                          className="tc-button tc-button--primary"
                          disabled={!state.capabilities.setProviderKey}
                          onClick={() => handleInlineSave(model)}
                          type="button"
                        >
                          {t("common.save")}
                        </button>
                      </div>
                    </article>
                  );
                })
              )}
            </section>
          </div>
        )}

        {isFormOpen ? (
          <div
            className="tc-settings-modal"
            onClick={deletingModelId ? undefined : closeForm}
          >
            <section
              aria-labelledby="settings-model-form-title"
              aria-modal="true"
              className="tc-card tc-settings-modal__card"
              data-testid="settings-model-form"
              onClick={(event) => event.stopPropagation()}
              role="dialog"
            >
              <div className="tc-settings-modal__header">
                <div>
                  <h3 id="settings-model-form-title">{formTitle}</h3>
                  <p>{formDescription}</p>
                </div>
                <button
                  aria-label={t("models.closeForm")}
                  className="tc-icon-button tc-settings-modal__close"
                  data-testid="settings-close-model-form"
                  disabled={Boolean(deletingModelId)}
                  onClick={closeForm}
                  type="button"
                >
                  <span aria-hidden="true" className="codicon codicon-close" />
                </button>
              </div>

              {validationError ? (
                <div className="tc-banner tc-banner--warning">
                  {t(validationError.key, validationError.args)}
                </div>
              ) : null}

              {deletingModelId ? (
                <div className="tc-banner" role="status">
                  {t("models.deleting", { name: deletingModelId })}
                </div>
              ) : null}
              {builtinCollision ? (
                <div className="tc-banner tc-banner--warning">
                  {t("models.builtinCollision", { name: builtinCollision.id })}
                </div>
              ) : null}

              <div className="tc-settings-form">
                <div
                  aria-label={t("models.mode")}
                  className="tc-settings-tabs"
                  role="tablist"
                >
                  <button
                    aria-controls={panelIdForDialogKind("official")}
                    aria-selected={dialogKind === "official"}
                    className={`tc-settings-tabs__tab${dialogKind === "official" ? " tc-settings-tabs__tab--active" : ""}`}
                    id={tabIdForDialogKind("official")}
                    onClick={() => handleDialogKindChange("official")}
                    onKeyDown={(event) =>
                      handleDialogTabKeyDown("official", event)
                    }
                    role="tab"
                    tabIndex={dialogKind === "official" ? 0 : -1}
                    type="button"
                  >
                    {t("models.official")}
                  </button>
                  <button
                    aria-controls={panelIdForDialogKind("relay")}
                    aria-selected={dialogKind === "relay"}
                    className={`tc-settings-tabs__tab${dialogKind === "relay" ? " tc-settings-tabs__tab--active" : ""}`}
                    data-testid="settings-mode-relay"
                    id={tabIdForDialogKind("relay")}
                    onClick={() => handleDialogKindChange("relay")}
                    onKeyDown={(event) =>
                      handleDialogTabKeyDown("relay", event)
                    }
                    role="tab"
                    tabIndex={dialogKind === "relay" ? 0 : -1}
                    type="button"
                  >
                    {t("models.relay")}
                  </button>
                </div>

                {dialogKind === "official" ? (
                  <div
                    aria-labelledby={tabIdForDialogKind("official")}
                    className="tc-settings-tabpanel"
                    id={panelIdForDialogKind("official")}
                    role="tabpanel"
                  >
                    {selectedPreset ? (
                      <>
                        <div className="tc-settings-form__row">
                          <label className="tc-field">
                            <span>{t("models.provider")}</span>
                            <select
                              aria-label={t("models.provider")}
                              onChange={(event) =>
                                handlePresetChange(event.target.value)
                              }
                              value={selectedPreset?.provider ?? ""}
                            >
                              {providerPresets.map((preset) => (
                                <option
                                  key={preset.provider}
                                  value={preset.provider}
                                >
                                  {preset.label}
                                </option>
                              ))}
                            </select>
                            <small className="tc-field__hint">
                              {t("models.officialHint")}
                            </small>
                          </label>
                          <label className="tc-field">
                            <span>{t("models.name")}</span>
                            <input
                              className="tc-input"
                              onChange={(event) =>
                                setForm((current) => ({
                                  ...current,
                                  modelName: event.target.value,
                                }))
                              }
                              placeholder={t("models.example", { name: "gpt-5.6" })}
                              value={form.modelName ?? ""}
                            />
                            <small className="tc-field__hint">
                              {t("models.exactName")}
                            </small>
                          </label>
                        </div>

                        <div className="tc-settings-preset-summary">
                          <span className="tc-settings-preset-summary__line">
                            {formatApiLabel(selectedPreset.api, t)}
                          </span>
                          <span className="tc-settings-preset-summary__line">
                            {selectedPreset.baseUrl}
                          </span>
                          <span className="tc-settings-preset-summary__line">
                            {selectedPreset.apiKeyEnv}
                            {selectedPreset.keyPresent
                              ? t("models.alreadyConfigured")
                              : t("models.notConfigured")}
                          </span>
                        </div>
                      </>
                    ) : (
                      <div className="tc-settings-mode-empty" role="status">
                        <p>{t("models.noPresets")}</p>
                        <button
                          className="tc-button tc-button--secondary"
                          onClick={() => handleDialogKindChange("relay")}
                          type="button"
                        >
                          {t("models.useRelay")}
                        </button>
                      </div>
                    )}
                  </div>
                ) : (
                  <div
                    aria-labelledby={tabIdForDialogKind("relay")}
                    className="tc-settings-tabpanel"
                    id={panelIdForDialogKind("relay")}
                    role="tabpanel"
                  >
                    <label className="tc-field">
                      <span>{t("term.baseUrl")}</span>
                      <input
                        className="tc-input"
                        onChange={(event) =>
                          setForm((current) => ({
                            ...current,
                            baseUrl: event.target.value,
                          }))
                        }
                        placeholder="https://api.chatanywhere.tech/v1"
                        value={form.baseUrl ?? ""}
                      />
                      <small className="tc-field__hint">
                        {t("models.relayHint")}
                      </small>
                    </label>

                    <div className="tc-settings-form__row">
                      <label className="tc-field">
                        <span>{t("term.api")}</span>
                        <select
                          aria-label={t("term.api")}
                          onChange={(event) =>
                            handleApiChange(event.target.value)
                          }
                          value={fieldText(form.api) || "openai"}
                        >
                          {API_OPTIONS.map((entry) => (
                            <option key={entry.value} value={entry.value}>
                              {t(entry.labelKey)}
                            </option>
                          ))}
                        </select>
                        <small className="tc-field__hint">
                          {t("models.apiHint")}
                        </small>
                      </label>
                      <label className="tc-field">
                        <span>{t("models.name")}</span>
                        <input
                          className="tc-input"
                          onChange={(event) =>
                            setForm((current) => ({
                              ...current,
                              modelName: event.target.value,
                            }))
                          }
                          placeholder={t("models.example", { name: "gpt-5.4" })}
                          value={form.modelName ?? ""}
                        />
                        <small className="tc-field__hint">
                          {t("models.relayNameHint")}
                        </small>
                      </label>
                    </div>

                    <div className="tc-settings-preview">
                      <div className="tc-settings-preview__title">
                        {t("models.willSaveAs")}
                      </div>
                      <div className="tc-settings-preview__row">
                        <span>provider</span>
                        <strong>{effectiveProvider || "—"}</strong>
                      </div>
                      <div className="tc-settings-preview__row">
                        <span>env</span>
                        <strong>{effectiveApiKeyEnv || "—"}</strong>
                      </div>
                      <div className="tc-settings-preview__row">
                        <span>id</span>
                        <strong>{effectiveId || "—"}</strong>
                      </div>
                    </div>
                    {automaticReusableModel ? (
                      <div
                        className="tc-settings-derived-model"
                        data-testid="settings-builtin-capability-source"
                      >
                        <strong>
                          {t(automaticReusableModel.source === "user" ? "models.reusingConfigured" : "models.reusingBuiltin", { name: automaticReusableModel.id })}
                        </strong>
                        <span>
                          {t("models.reusingHint")}
                        </span>
                      </div>
                    ) : null}
                  </div>
                )}

                <label className="tc-field">
                  <span>{t("models.alias")}</span>
                  <input
                    aria-label={t("models.alias")}
                    className={`tc-input${formMode === "edit" ? " tc-input--readonly" : ""}`}
                    onChange={(event) => {
                      if (formMode === "edit") {
                        return;
                      }
                      setIdManuallyEdited(true);
                      setForm((current) => ({
                        ...current,
                        id: event.target.value,
                      }));
                    }}
                    placeholder={derivedId || t("models.aliasDefault")}
                    readOnly={formMode === "edit"}
                    value={effectiveId}
                  />
                  <small className="tc-field__hint">
                    {formMode === "edit"
                      ? t("models.aliasFixed")
                      : t("models.aliasHint")}
                  </small>
                </label>

                {showSharedFormFields ? (
                  <>
                    <div
                      className="tc-settings-form__row"
                      data-testid="settings-key-fields-row"
                    >
                      <KeySlotCombobox
                        feedback={keySlotRefreshFeedback ? t(keySlotRefreshFeedback) : null}
                        hint={t("models.keySearchHint")}
                        onChange={handleKeySlotChange}
                        onRefresh={handleKeySlotRefresh}
                        options={keySlotOptions}
                        placeholder={suggestedApiKeyEnv || "EXAMPLE_API_KEY"}
                        refreshDisabled={
                          !state.capabilities.listProviderKeys ||
                          isKeySlotRefreshing
                        }
                        refreshLabel={t("models.keyRefresh")}
                        refreshing={isKeySlotRefreshing}
                        value={effectiveApiKeyEnv}
                      />
                      <label className="tc-field">
                        <div className="tc-field__label-row">
                          <span>
                            {effectiveKeyPresent
                              ? t("models.keyNew")
                              : t("term.field.apiKey")}
                          </span>
                        </div>
                        <input
                          aria-label={t("term.field.apiKey")}
                          autoComplete="off"
                          className="tc-input tc-settings-api-key-input"
                          data-testid="settings-api-key-input"
                          onBlur={() => setIsApiKeyFocused(false)}
                          onChange={(event) => {
                            setIsApiKeyFocused(true);
                            setDraftApiKey(event.target.value);
                          }}
                          onFocus={() => setIsApiKeyFocused(true)}
                          placeholder={
                            effectiveKeyPresent
                              ? t("models.leaveBlank", { name: effectiveApiKeyEnv })
                              : t("models.saveKey", { name: effectiveApiKeyEnv || t("models.selectedSlot") })
                          }
                          readOnly={!isApiKeyFocused && draftApiKey.length > 0}
                          type={
                            isApiKeyFocused || !draftApiKey
                              ? "password"
                              : "text"
                          }
                          value={
                            isApiKeyFocused || !draftApiKey
                              ? draftApiKey
                              : maskDraftApiKey(draftApiKey)
                          }
                        />
                        <small className="tc-field__hint">
                          {effectiveKeyPresent
                            ? t("models.alreadyKey", { name: effectiveApiKeyEnv })
                            : t("models.keyHint")}
                        </small>
                      </label>
                    </div>

                    <button
                      aria-expanded={showAdvanced}
                      className="tc-settings-advanced__toggle"
                      data-testid="settings-model-advanced"
                      onClick={() => setShowAdvanced((current) => !current)}
                      type="button"
                    >
                      <span>{t("models.advanced")}</span>
                      <span className="tc-settings-advanced__caret">
                        {showAdvanced ? "▾" : "▸"}
                      </span>
                    </button>

                    {showAdvanced ? (
                      <div className="tc-settings-advanced">
                        {dialogKind === "official" ? (
                          <div className="tc-settings-form__row">
                            <label className="tc-field">
                              <span>{t("models.apiOverride")}</span>
                              <select
                                aria-label={t("models.apiOverride")}
                                onChange={(event) =>
                                  handleApiChange(event.target.value)
                                }
                                value={
                                  fieldText(form.api) ||
                                  selectedPreset?.api ||
                                  ""
                                }
                              >
                                {API_OPTIONS.map((entry) => (
                                  <option key={entry.value} value={entry.value}>
                                    {t(entry.labelKey)}
                                  </option>
                                ))}
                              </select>
                            </label>
                            <label className="tc-field">
                              <span>{t("models.urlOverride")}</span>
                              <input
                                className="tc-input"
                                onChange={(event) =>
                                  setForm((current) => ({
                                    ...current,
                                    baseUrl: event.target.value,
                                  }))
                                }
                                placeholder={
                                  selectedPreset?.baseUrl ||
                                  "https://api.example.com/v1"
                                }
                                value={form.baseUrl ?? ""}
                              />
                            </label>
                          </div>
                        ) : null}

                        <div className="tc-settings-form__row">
                          <label className="tc-field">
                            <span>{t("models.providerOverride")}</span>
                            <input
                              className="tc-input"
                              onChange={(event) =>
                                setForm((current) => ({
                                  ...current,
                                  provider: event.target.value,
                                }))
                              }
                              placeholder={
                                effectiveProvider ||
                                t("models.derived")
                              }
                              value={form.provider}
                            />
                            <small className="tc-field__hint">
                              {t("models.providerHint")}
                            </small>
                          </label>
                        </div>

                        <div className="tc-settings-form__row">
                          <div className="tc-settings-auto-field">
                            <span>{t("models.contextWindow")}</span>
                            <strong data-testid="settings-context-window-auto">
                              {effectiveContextWindow}
                            </strong>
                            <small>
                              {t("models.contextHint")}
                            </small>
                          </div>
                          <div className="tc-settings-auto-field">
                            <span>{t("models.maxOutput")}</span>
                            <strong data-testid="settings-max-output-tokens-auto">
                              {effectiveMaxOutputTokens}
                            </strong>
                            <small className="tc-field__hint">
                              {t("models.maxOutputHint")}
                            </small>
                          </div>
                        </div>

                        <label className="tc-field">
                          <span>{t("models.thinkingFormat")}</span>
                          <select
                            aria-label={t("models.thinkingFormat")}
                            onChange={(event) =>
                              setForm((current) => ({
                                ...current,
                                thinkingFormat:
                                  dialogKind === "official" &&
                                  selectedPreset &&
                                  event.target.value ===
                                    selectedPreset.thinkingFormat
                                    ? ""
                                    : event.target.value,
                              }))
                            }
                            value={
                              fieldText(form.thinkingFormat) ||
                              (dialogKind === "official"
                                ? (selectedPreset?.thinkingFormat ??
                                  defaultThinkingFormatForApi(effectiveApi))
                                : defaultThinkingFormatForApi(effectiveApi))
                            }
                          >
                            {THINKING_FORMAT_OPTIONS.map((entry) => (
                              <option
                                key={entry.value || "auto"}
                                value={entry.value}
                              >
                                {t(entry.labelKey)}
                              </option>
                            ))}
                          </select>
                          <small className="tc-field__hint">
                            {t("models.thinkingHint")}
                          </small>
                        </label>

                        <div className="tc-settings-form__row">
                          <label className="tc-field">
                            <span>{t("models.descriptionField")}</span>
                            <input
                              className="tc-input"
                              onChange={(event) =>
                                setForm((current) => ({
                                  ...current,
                                  description: normalizeOptionalText(
                                    event.target.value,
                                  ),
                                }))
                              }
                              placeholder={t("models.descriptionOptional")}
                              value={form.description ?? ""}
                            />
                          </label>
                          <label className="tc-field">
                            <span>{t("models.effortLevels")}</span>
                            <input
                              className="tc-input"
                              onChange={(event) =>
                                setForm((current) => ({
                                  ...current,
                                  reasoningLevelsText: event.target.value,
                                }))
                              }
                              placeholder="low, medium, high, xhigh, max"
                              data-testid="settings-supported-effort-levels"
                              aria-label={t("models.effortLevels")}
                              value={form.reasoningLevelsText ?? ""}
                            />
                            <small className="tc-field__hint">
                              {t("models.effortHint")}
                            </small>
                          </label>
                        </div>

                        <label className="tc-field">
                          <span>{t("models.speeds")}</span>
                          <input
                            className="tc-input"
                            data-testid="settings-supported-speeds"
                            aria-label={t("models.speeds")}
                            onChange={(event) => setForm((current) => ({ ...current, speedsText: event.target.value }))}
                            placeholder={t("models.speedsExample")}
                            value={form.speedsText ?? ""}
                          />
                          <small className="tc-field__hint">
                            {t("models.speedsHint")}
                          </small>
                        </label>

                        <div className="tc-settings-capabilities">
                          {CAPABILITY_OPTIONS.map(([key, label]) => (
                            <label
                              className="tc-settings-capabilities__item"
                              key={key}
                            >
                              <input
                                checked={form.capabilities[key]}
                                onChange={(event) =>
                                  handleCapabilityChange(
                                    key,
                                    event.target.checked,
                                  )
                                }
                                type="checkbox"
                              />
                              <span>{t(label)}</span>
                            </label>
                          ))}
                        </div>
                      </div>
                    ) : null}
                  </>
                ) : null}

                <div className="tc-button-row tc-settings-form__actions">
                  <button
                    className="tc-button tc-button--ghost"
                    disabled={Boolean(deletingModelId)}
                    onClick={closeForm}
                    type="button"
                  >
                    {t("common.cancel")}
                  </button>
                  {selectedModel?.source === "user" ? (
                    <button
                      className="tc-button tc-button--ghost"
                      disabled={!state.capabilities.removeModel || Boolean(deletingModelId)}
                      onClick={handleDelete}
                      type="button"
                    >
                      {t("models.delete")}
                    </button>
                  ) : null}
                  <button
                    className="tc-button tc-button--primary"
                    disabled={saveDisabled || Boolean(deletingModelId)}
                    data-testid="settings-save-model"
                    onClick={handleSave}
                    type="button"
                  >
                    {t("models.saveModel")}
                  </button>
                </div>
                {saveDisabledReason ? (
                  <div className="tc-settings-form__disabled-reason">
                    {saveDisabledReason}
                  </div>
                ) : null}
              </div>
            </section>
          </div>
        ) : null}
        {deleteConfirmation ? (
          <div className="tc-settings-modal" role="presentation">
            <section
              aria-labelledby="delete-model-title"
              aria-modal="true"
              className="tc-card tc-settings-modal__card"
              data-testid="settings-delete-model-confirmation"
              role="alertdialog"
            >
              <div className="tc-settings-modal__header">
                <div>
                  <h3 id="delete-model-title">{t("models.deleteQuestion", { name: deleteConfirmation })}</h3>
                  <p>{t("models.deleteHint")}</p>
                </div>
              </div>
              <div className="tc-button-row tc-settings-form__actions">
                <button
                  className="tc-button tc-button--ghost"
                  onClick={() => setDeleteConfirmation(null)}
                  type="button"
                >
                  Cancel
                </button>
                <button
                  className="tc-button tc-button--danger"
                  onClick={confirmDelete}
                  type="button"
                >
                  {t("models.deleteAction")}
                </button>
              </div>
            </section>
          </div>
        ) : null}
        {replacementConfirmation ? (
          <div className="tc-settings-modal" role="presentation">
            <section
              aria-labelledby="replace-shared-key-title"
              aria-modal="true"
              className="tc-card tc-settings-modal__card"
              role="alertdialog"
            >
              <div className="tc-settings-modal__header">
                <div>
                  <h3 id="replace-shared-key-title">{t("models.replaceQuestion")}</h3>
                  <p>{t("models.replaceHint", { name: replacementConfirmation.envName })}</p>
                </div>
              </div>
              <div className="tc-settings-form">
                <p>{t("models.replaceAffected")}</p>
                <ul>
                  {replacementConfirmation.modelIds.map((modelId) => (
                    <li key={modelId}>{modelId}</li>
                  ))}
                </ul>
                <div className="tc-button-row tc-settings-form__actions">
                  <button
                    className="tc-button tc-button--ghost"
                    onClick={() => setReplacementConfirmation(null)}
                    type="button"
                  >
                    {t("common.cancel")}
                  </button>
                  <button
                    className="tc-button tc-button--primary"
                    onClick={() => {
                      setReplacementConfirmation(null);
                      submitModelSave();
                    }}
                    type="button"
                  >
                    {t("models.replaceAction")}
                  </button>
                </div>
              </div>
            </section>
          </div>
        ) : null}
      </main>
    </SettingsShell>
  );
}
