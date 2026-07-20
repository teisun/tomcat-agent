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
import { deriveRelayFields, RELAY_ID_SEPARATOR } from "./relayDerive";
import { KeySlotCombobox, type KeySlotOption } from "./KeySlotCombobox";
import { isValidKeySlotName } from "./keySlot";

type FormState = SettingsModelInput;
type FormMode = "create" | "edit";
type DialogKind = "official" | "relay";
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
  { label: "OpenAI (Chat Completions)", value: "openai" },
  { label: "OpenAI (Responses)", value: "openai-responses" },
  { label: "Anthropic (Messages)", value: "anthropic-messages" },
] as const;

const THINKING_FORMAT_OPTIONS = [
  { label: "OpenAI effort", value: "openai" },
  { label: "DeepSeek thinking", value: "deepseek" },
  { label: "ZAI / GLM reasoning", value: "zai" },
  { label: "Doubao / Kimi / MiMo thinking", value: "doubao" },
  { label: "Anthropic thinking budget", value: "anthropic" },
  { label: "Anthropic adaptive effort", value: "anthropic-adaptive" },
] as const;

const CAPABILITY_OPTIONS = [
  ["vision", "Vision"],
  ["files", "Files"],
  ["tools", "Tools"],
  ["reasoning", "Reasoning"],
  ["webSearch", "Web Search"],
] as const;

function cloneCapabilities(
  capabilities: SettingsModelCapabilities,
): SettingsModelCapabilities {
  return { ...capabilities };
}

function createEmptyForm(): FormState {
  return {
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

function normalizeOptionalText(value: string | null | undefined): string | null {
  const trimmed = fieldText(value);
  return trimmed ? trimmed : null;
}

function hasScheme(value: string): boolean {
  return /^[a-z][a-z0-9+.-]*:\/\//i.test(value);
}

function normalizeBaseUrlInput(value: string | null | undefined): string | null {
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
    contextWindow:
      typeof model.contextWindow === "number" && Number.isFinite(model.contextWindow)
        ? model.contextWindow
        : null,
    id: fieldText(model.id),
    modelName: normalizeOptionalText(model.modelName),
    provider: fieldText(model.provider),
    supportedReasoningLevels: Array.isArray(model.supportedReasoningLevels)
      ? [...model.supportedReasoningLevels]
      : null,
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

function frameIsTestEvent(
  message: unknown,
): message is {
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
    typeof (message as { content?: { type?: unknown } }).content?.type === "string"
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
  return { height: rect.height, left: rect.left, top: rect.top, width: rect.width };
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
  const target = document.querySelector<HTMLElement>(`[data-testid="${action.testId}"]`);
  if (!(target instanceof HTMLElement)) {
    return;
  }
  if (action.kind === "clickTestId") {
    target.click();
    return;
  }
  if (action.kind === "setInputValue" && target instanceof HTMLInputElement) {
    target.value = action.value ?? "";
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
    id: model.id,
    modelName: model.modelName ?? "",
    provider: model.provider,
    supportedReasoningLevels: model.supportedReasoningLevels ?? null,
    thinkingFormat: model.thinkingFormat ?? "",
  };
}

function formatApiLabel(api: string): string {
  return API_OPTIONS.find((entry) => entry.value === api)?.label ?? api;
}

function formatThinkingLabel(thinkingFormat: string | null | undefined): string {
  return (
    THINKING_FORMAT_OPTIONS.find((entry) => entry.value === (thinkingFormat ?? ""))?.label ??
    thinkingFormat ??
    ""
  );
}

function formatVersionLabel(version: string | null | undefined): string {
  const trimmed = fieldText(version);
  return trimmed ? `v${trimmed}` : "vunknown";
}

function buildServeVersionWarning(state: SettingsStateSnapshot): string | null {
  const expected = fieldText(state.expectedCliVersion);
  const server = fieldText(state.serverVersion);
  if (!server) {
    return "The connected `tomcat serve` did not report a version. You may be running an older CLI binary; rebuild or update it, then restart serve.";
  }
  if (expected && server !== expected) {
    return `This extension expects tomcat CLI v${expected}, but the connected serve reports v${server}. Rebuild or update the CLI binary, then restart serve.`;
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

function buildModelDetails(model: SettingsModelView): Array<{ label: string; value: string }> {
  const detailRows = [
    {
      label: "Source",
      value: model.source === "user" ? "User" : "Built-in",
    },
    {
      label: "API",
      value: formatApiLabel(model.api),
    },
    {
      label: "Provider",
      value: model.provider,
    },
    {
      label: "API Key Env",
      value: modelKeyEnvName(model),
    },
    {
      label: "Base URL",
      value: model.baseUrl ?? "",
    },
    {
      label: "Thinking",
      value: formatThinkingLabel(model.thinkingFormat),
    },
    {
      label: "Context Window",
      value:
        typeof model.contextWindow === "number"
          ? String(model.contextWindow)
          : "",
    },
    {
      label: "Model Name",
      value: model.modelName ?? "",
    },
  ];
  return detailRows.filter((entry) => entry.value.trim().length > 0);
}

function buildConfiguredKeyLabel(entry: SettingsProviderKeyView): string {
  return entry.keyPresent ? `${entry.envName} (configured)` : entry.envName;
}

function buildKeySlotOptions(
  suggestedEnvName: string,
  providerKeys: SettingsProviderKeyView[],
): KeySlotOption[] {
  const options: KeySlotOption[] = [];
  const seen = new Set<string>();
  if (suggestedEnvName) {
    const existing = providerKeys.find((entry) => entry.envName === suggestedEnvName);
    options.push({
      envName: suggestedEnvName,
      group: "suggested",
      keyPresent: existing?.keyPresent ?? false,
      label: existing?.keyPresent
        ? `${suggestedEnvName} (configured)`
        : `${suggestedEnvName} (suggested)`,
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
      label: buildConfiguredKeyLabel(entry),
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

function send(
  vscodeApi: VsCodeApiLike<SettingsIntent>,
  message: Omit<SettingsIntent, "messageId">,
): void {
  vscodeApi.postMessage({
    ...message,
    messageId: `${message.type}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
  } as SettingsIntent);
}

export function SettingsApp({
  vscodeApi,
}: {
  vscodeApi: VsCodeApiLike<SettingsIntent>;
}) {
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
    route: "models",
  });
  const [dialogKind, setDialogKind] = useState<DialogKind>("official");
  const [form, setForm] = useState<FormState>(() => createEmptyForm());
  const [draftApiKey, setDraftApiKey] = useState("");
  const [isApiKeyFocused, setIsApiKeyFocused] = useState(false);
  const [inlineApiKeys, setInlineApiKeys] = useState<Record<string, string>>({});
  const [selectedModelId, setSelectedModelId] = useState<string | null>(null);
  const [selectedProvider, setSelectedProvider] = useState("");
  const [formMode, setFormMode] = useState<FormMode>("create");
  const [isFormOpen, setIsFormOpen] = useState(false);
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [validationError, setValidationError] = useState<string | null>(null);
  const [isKeySlotRefreshing, setIsKeySlotRefreshing] = useState(false);
  const [keySlotRefreshFeedback, setKeySlotRefreshFeedback] = useState<string | null>(null);
  const [replacementConfirmation, setReplacementConfirmation] = useState<{
    envName: string;
    modelIds: string[];
  } | null>(null);
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
        setKeySlotRefreshFeedback(event.data.content.error ? null : "Key slots refreshed.");
      }
      setState(event.data.content);
    };
    window.addEventListener("message", handleMessage);
    send(vscodeApi, {
      data: {
        route: "models",
      },
      type: "settings.ready",
    });
    return () => {
      window.removeEventListener("message", handleMessage);
    };
  }, [vscodeApi]);

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
    if (!isFormOpen || dialogKind !== "official" || selectedProvider || providerPresets.length === 0) {
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
      ? providerPresetByProvider.get(selectedProvider) ?? providerPresets[0] ?? null
      : null;
  const relayDerived = useMemo(
    () =>
      deriveRelayFields(
        form.baseUrl ?? "",
        form.modelName ?? "",
        fieldText(form.api) || "openai",
        RELAY_ID_SEPARATOR,
      ),
    [form.api, form.baseUrl, form.modelName],
  );

  const effectiveModelName = fieldText(form.modelName);
  const effectiveApi =
    dialogKind === "official"
      ? fieldText(form.api) || selectedPreset?.api || ""
      : fieldText(form.api) || "openai";
  const effectiveProvider =
    dialogKind === "official"
      ? fieldText(form.provider) || selectedPreset?.provider || ""
      : fieldText(form.provider) || relayDerived.provider;
  const effectiveBaseUrl =
    dialogKind === "official"
      ? normalizeBaseUrlInput(form.baseUrl) ?? selectedPreset?.baseUrl ?? null
      : normalizeBaseUrlInput(form.baseUrl);
  const suggestedApiKeyEnv =
    dialogKind === "official"
      ? selectedPreset?.apiKeyEnv ?? ""
      : relayDerived.apiKeyEnv;
  const effectiveApiKeyEnv = fieldText(form.apiKeyEnv) || suggestedApiKeyEnv;
  const derivedId = dialogKind === "official" ? effectiveModelName : relayDerived.id;
  const effectiveId = selectedModel ? selectedModel.id : fieldText(form.id) || derivedId;
  const effectiveThinkingFormat =
    fieldText(form.thinkingFormat) ||
    (dialogKind === "official"
      ? selectedPreset?.thinkingFormat ?? defaultThinkingFormatForApi(effectiveApi)
      : defaultThinkingFormatForApi(effectiveApi));
  const effectiveModel: SettingsModelInput = normalizeModel({
    ...form,
    api: effectiveApi,
    apiKeyEnv: effectiveApiKeyEnv,
    baseUrl: effectiveBaseUrl ?? "",
    id: effectiveId,
    modelName: effectiveModelName,
    provider: effectiveProvider,
    thinkingFormat: effectiveThinkingFormat,
  });

  const keySlotOptions = useMemo(
    () => buildKeySlotOptions(suggestedApiKeyEnv, state.providerKeys),
    [state.providerKeys, suggestedApiKeyEnv],
  );
  const selectedKeyStatus = effectiveApiKeyEnv
    ? providerKeysByEnv.get(effectiveApiKeyEnv) ?? null
    : null;
  const effectiveKeyPresent = Boolean(selectedKeyStatus?.keyPresent);
  const builtinCollision = useMemo(
    () =>
      state.models.find(
        (model) => model.source === "builtin" && model.id === effectiveId,
      ) ?? null,
    [effectiveId, state.models],
  );
  const normalizedContextWindow =
    typeof form.contextWindow === "number" && Number.isFinite(form.contextWindow)
      ? form.contextWindow
      : null;
  const officialPresetUnavailable =
    dialogKind === "official" && selectedPreset === null;
  const showSharedFormFields = dialogKind === "relay" || selectedPreset !== null;

  function resetForm() {
    const nextDialogKind = defaultDialogKindForPresets(providerPresets);
    setDialogKind(nextDialogKind);
    setDraftApiKey("");
    setIsApiKeyFocused(false);
    setIsKeySlotRefreshing(false);
    setKeySlotRefreshFeedback(null);
    setSelectedModelId(null);
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
        ? providerPresetByProvider.get(inferred.selectedProvider) ?? null
        : null;
    setSelectedModelId(model.id);
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
      const nextProvider = selectedProvider || providerPresets[0]?.provider || "";
      const preset = providerPresetByProvider.get(nextProvider) ?? providerPresets[0] ?? null;
      setSelectedProvider(nextProvider);
      setForm((current) => ({
        ...createEmptyForm(),
        capabilities: cloneCapabilities(
          preset?.capabilities ?? RELAY_DEFAULT_CAPABILITIES,
        ),
        contextWindow: current.contextWindow ?? null,
        id: selectedModel ? selectedModel.id : "",
        modelName: current.modelName ?? "",
      }));
      return;
    }

    setForm((current) => ({
      ...createEmptyForm(),
      api: fieldText(current.api) || "openai",
      baseUrl: current.baseUrl ?? "",
      capabilities: cloneCapabilities(RELAY_DEFAULT_CAPABILITIES),
      contextWindow: current.contextWindow ?? null,
      id: selectedModel ? selectedModel.id : "",
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
      const currentDefaultThinkingFormat = defaultThinkingFormatForApi(currentApi);
      const currentThinkingFormat = fieldText(current.thinkingFormat);
      const nextThinkingFormat =
        !currentThinkingFormat || currentThinkingFormat === currentDefaultThinkingFormat
          ? defaultThinkingFormatForApi(nextApi)
          : current.thinkingFormat;
      return {
        ...current,
        api:
          dialogKind === "official" && selectedPreset && nextApi === selectedPreset.api
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
    if (dialogKind === "official" && !selectedPreset) {
      setValidationError("Choose an official provider preset first.");
      return;
    }
    if (!effectiveModelName) {
      setValidationError("Model name is required. Example: gpt-5.6");
      return;
    }
    if (dialogKind === "relay" && !fieldText(form.baseUrl)) {
      setValidationError("Base URL is required for relay or custom endpoints.");
      return;
    }
    if (dialogKind === "relay" && !isValidBaseUrl(form.baseUrl)) {
      setValidationError(
        "Base URL could not be parsed. Use a value like https://host/v1.",
      );
      return;
    }
    if (!effectiveModel.id || !effectiveModel.provider || !effectiveModel.api) {
      setValidationError("Model ID, provider, and API are all required.");
      return;
    }
    if (!effectiveApiKeyEnv) {
      setValidationError("Choose or derive an API key slot before saving.");
      return;
    }
    if (!isValidKeySlotName(effectiveApiKeyEnv)) {
      setValidationError(
        "Key slot must match ^[A-Z_][A-Z0-9_]*$ (uppercase letters, numbers, and underscores).",
      );
      return;
    }
    if (!effectiveKeyPresent && !draftApiKey.trim()) {
      setValidationError(
        `Add an API key or switch to a configured slot such as ${effectiveApiKeyEnv}.`,
      );
      return;
    }
    if (draftApiKey.trim() && !state.capabilities.setProviderKey) {
      setValidationError("当前后端不支持保存 API Key，请先升级 `tomcat serve`。");
      return;
    }
    setValidationError(null);
    const affectedModelIds = state.models
      .filter(
        (model) =>
          model.apiKeyEnv === effectiveApiKeyEnv && model.id !== selectedModel?.id,
      )
      .map((model) => model.id);
    if (effectiveKeyPresent && draftApiKey.trim() && affectedModelIds.length > 0) {
      setReplacementConfirmation({
        envName: effectiveApiKeyEnv,
        modelIds: affectedModelIds,
      });
      return;
    }
    submitModelSave();
  }

  function handleDelete() {
    if (!selectedModel || selectedModel.source !== "user") {
      return;
    }
    send(vscodeApi, {
      data: {
        modelId: selectedModel.id,
      },
      type: "removeModel",
    });
    closeForm();
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
    formMode === "edit" && selectedModel ? `Edit ${selectedModel.id}` : "Add Model";
  const formDescription =
    formMode === "edit"
      ? "Update this model, reuse a saved key slot, or rotate the key without echoing it back into the UI."
      : "Add an official model that Tomcat does not ship yet, or connect a relay or custom endpoint.";

  const saveDisabledReason =
    !state.capabilities.upsertModel
      ? "The connected `tomcat serve` does not expose model writes yet."
      : officialPresetUnavailable
        ? "No official provider presets are available right now. Switch to Relay / custom endpoint."
        : !effectiveModelName
          ? "Enter a model name to continue."
          : !effectiveApiKeyEnv
            ? "Choose or derive an API key slot before saving."
            : !isValidKeySlotName(effectiveApiKeyEnv)
              ? "Key slot must match ^[A-Z_][A-Z0-9_]*$."
              : dialogKind === "relay" && !fieldText(form.baseUrl)
              ? "Enter a base URL to continue."
              : !effectiveKeyPresent && !draftApiKey.trim()
                ? `Add an API key or choose a configured slot such as ${effectiveApiKeyEnv}.`
                : null;
  const saveDisabled = saveDisabledReason !== null;
  const serveVersionWarning = buildServeVersionWarning(state);

  return (
    <div className="tc-settings-shell">
      <aside className="tc-settings-shell__nav">
        <div className="tc-settings-shell__brand">Tomcat Settings</div>
        <button
          className="tc-settings-nav__item tc-settings-nav__item--active"
          type="button"
        >
          Models
        </button>
        <button className="tc-settings-nav__item" disabled type="button">
          Sessions
        </button>
        <button className="tc-settings-nav__item" disabled type="button">
          Tools
        </button>
        <div className="tc-settings-shell__version" data-testid="settings-version-footer">
          <div>Extension {formatVersionLabel(state.extensionVersion)}</div>
          <div>Serve {formatVersionLabel(state.serverVersion)}</div>
        </div>
      </aside>
      <main className="tc-settings-shell__content">
        <header className="tc-settings-shell__header">
          <div>
            <h1>Models</h1>
            <p>
              Built-in and custom models, with provider keys stored without
              echoing them back into the UI.
            </p>
          </div>
          <button
            className="tc-button tc-button--secondary"
            data-testid="settings-add-model"
            disabled={!state.capabilities.upsertModel}
            onClick={openCreateForm}
            type="button"
          >
            + Add Model
          </button>
        </header>

        {serveVersionWarning ? (
          <div className="tc-banner tc-banner--warning">{serveVersionWarning}</div>
        ) : null}
        {state.error ? <div className="tc-banner tc-banner--warning">{state.error}</div> : null}
        {state.warnings?.map((warning) => (
          <div key={warning} className="tc-banner tc-banner--warning">
            {warning}
          </div>
        ))}
        {state.status ? <div className="tc-banner">{state.status}</div> : null}

        {!state.capabilities.listModels ? (
          <section className="tc-empty-state">
            <h2>Model management unavailable</h2>
            <p>
              The connected `tomcat serve` does not expose model management yet.
            </p>
          </section>
        ) : (
          <div className="tc-settings-groups">
            <section className="tc-settings-group">
              <h2 className="tc-settings-group__title">Ready</h2>
              {readyModels.length === 0 ? (
                <div className="tc-session-dropdown__empty">No ready models yet.</div>
              ) : (
                readyModels.map((model) => {
                  const details = buildModelDetails(model);
                  return (
                    <article className="tc-settings-model" key={model.id}>
                      <div className="tc-settings-model__header">
                        <div className="tc-settings-model__identity">
                          <span
                            aria-label="Ready"
                            className="tc-settings-model__status-dot tc-settings-model__status-dot--ready"
                            role="img"
                          />
                          <strong>{model.id}</strong>
                        </div>
                        <div className="tc-settings-model__actions">
                          <div className="tc-settings-model__tooltip-anchor">
                            <button
                              aria-label={`Show details for ${model.id}`}
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
                                    key={`${model.id}-${entry.label}`}
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
                            onClick={() => openEditForm(model)}
                            type="button"
                          >
                            Edit
                          </button>
                        </div>
                      </div>
                    </article>
                  );
                })
              )}
            </section>

            <section className="tc-settings-group">
              <h2 className="tc-settings-group__title">Needs API Key</h2>
              {needsKeyModels.length === 0 ? (
                <div className="tc-session-dropdown__empty">
                  All visible models are ready.
                </div>
              ) : (
                needsKeyModels.map((model) => {
                  const details = buildModelDetails(model);
                  return (
                    <article className="tc-settings-model" key={model.id}>
                      <div className="tc-settings-model__header">
                        <div className="tc-settings-model__identity">
                          <span
                            aria-label="Needs API key"
                            className="tc-settings-model__status-dot tc-settings-model__status-dot--missing"
                            role="img"
                          />
                          <strong>{model.id}</strong>
                        </div>
                        <div className="tc-settings-model__actions">
                          <div className="tc-settings-model__tooltip-anchor">
                            <button
                              aria-label={`Show details for ${model.id}`}
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
                                    key={`${model.id}-${entry.label}`}
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
                            onClick={() => openEditForm(model)}
                            type="button"
                          >
                            Edit
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
                          placeholder={`Save ${modelKeyEnvName(model)}`}
                          type="password"
                          value={inlineApiKeys[model.id] ?? ""}
                        />
                        <button
                          className="tc-button tc-button--primary"
                          disabled={!state.capabilities.setProviderKey}
                          onClick={() => handleInlineSave(model)}
                          type="button"
                        >
                          Save
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
          <div className="tc-settings-modal" onClick={closeForm}>
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
                  aria-label="Close model form"
                  className="tc-icon-button tc-settings-modal__close"
                  data-testid="settings-close-model-form"
                  onClick={closeForm}
                  type="button"
                >
                  <span aria-hidden="true" className="codicon codicon-close" />
                </button>
              </div>

              {validationError ? (
                <div className="tc-banner tc-banner--warning">{validationError}</div>
              ) : null}
              {builtinCollision ? (
                <div className="tc-banner tc-banner--warning">
                  Saving this model will override the built-in model `{builtinCollision.id}`.
                </div>
              ) : null}

              <div className="tc-settings-form">
                <div
                  aria-label="Add model mode"
                  className="tc-settings-tabs"
                  role="tablist"
                >
                  <button
                    aria-controls={panelIdForDialogKind("official")}
                    aria-selected={dialogKind === "official"}
                    className={`tc-settings-tabs__tab${dialogKind === "official" ? " tc-settings-tabs__tab--active" : ""}`}
                    id={tabIdForDialogKind("official")}
                    onClick={() => handleDialogKindChange("official")}
                    onKeyDown={(event) => handleDialogTabKeyDown("official", event)}
                    role="tab"
                    tabIndex={dialogKind === "official" ? 0 : -1}
                    type="button"
                  >
                    Official new model
                  </button>
                  <button
                    aria-controls={panelIdForDialogKind("relay")}
                    aria-selected={dialogKind === "relay"}
                    className={`tc-settings-tabs__tab${dialogKind === "relay" ? " tc-settings-tabs__tab--active" : ""}`}
                    data-testid="settings-mode-relay"
                    id={tabIdForDialogKind("relay")}
                    onClick={() => handleDialogKindChange("relay")}
                    onKeyDown={(event) => handleDialogTabKeyDown("relay", event)}
                    role="tab"
                    tabIndex={dialogKind === "relay" ? 0 : -1}
                    type="button"
                  >
                    Relay / custom endpoint
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
                            <span>Provider</span>
                            <select
                              aria-label="Provider"
                              onChange={(event) => handlePresetChange(event.target.value)}
                              value={selectedPreset?.provider ?? ""}
                            >
                              {providerPresets.map((preset) => (
                                <option key={preset.provider} value={preset.provider}>
                                  {preset.label}
                                </option>
                              ))}
                            </select>
                            <small className="tc-field__hint">
                              Choose the official vendor. Tomcat fills in the API,
                              URL, key slot, thinking format, and capabilities.
                            </small>
                          </label>
                          <label className="tc-field">
                            <span>Model name</span>
                            <input
                              className="tc-input"
                              onChange={(event) =>
                                setForm((current) => ({
                                  ...current,
                                  modelName: event.target.value,
                                }))
                              }
                              placeholder="For example: gpt-5.6"
                              value={form.modelName ?? ""}
                            />
                            <small className="tc-field__hint">
                              The exact model name the provider expects.
                            </small>
                          </label>
                        </div>

                        <label className="tc-field">
                          <span>Model ID (alias)</span>
                          <input
                            className="tc-input tc-input--readonly"
                            disabled
                            readOnly
                            value={effectiveId}
                          />
                          <small className="tc-field__hint">
                            This is how the model appears inside Tomcat. You can
                            override the alias in Advanced.
                          </small>
                        </label>

                        <div className="tc-settings-preset-summary">
                          <span className="tc-settings-preset-summary__line">
                            {formatApiLabel(selectedPreset.api)}
                          </span>
                          <span className="tc-settings-preset-summary__line">
                            {selectedPreset.baseUrl}
                          </span>
                          <span className="tc-settings-preset-summary__line">
                            {selectedPreset.apiKeyEnv}
                            {selectedPreset.keyPresent ? " already configured" : " not configured yet"}
                          </span>
                        </div>
                      </>
                    ) : (
                      <div className="tc-settings-mode-empty" role="status">
                        <p>
                          No official provider presets are available from the
                          connected `tomcat serve`.
                        </p>
                        <button
                          className="tc-button tc-button--secondary"
                          onClick={() => handleDialogKindChange("relay")}
                          type="button"
                        >
                          Use Relay / custom endpoint
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
                      <span>Base URL</span>
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
                        The relay or custom endpoint. A missing scheme will be
                        saved as `https://...`.
                      </small>
                    </label>

                    <div className="tc-settings-form__row">
                      <label className="tc-field">
                        <span>API</span>
                        <select
                          aria-label="API"
                          onChange={(event) => handleApiChange(event.target.value)}
                          value={fieldText(form.api) || "openai"}
                        >
                          {API_OPTIONS.map((entry) => (
                            <option key={entry.value} value={entry.value}>
                              {entry.label}
                            </option>
                          ))}
                        </select>
                        <small className="tc-field__hint">
                          This decides how Tomcat talks to the endpoint and how
                          reasoning effort is encoded.
                        </small>
                      </label>
                      <label className="tc-field">
                        <span>Model name</span>
                        <input
                          className="tc-input"
                          onChange={(event) =>
                            setForm((current) => ({
                              ...current,
                              modelName: event.target.value,
                            }))
                          }
                          placeholder="For example: gpt-5.4"
                          value={form.modelName ?? ""}
                        />
                        <small className="tc-field__hint">
                          The exact model name your relay forwards upstream.
                        </small>
                      </label>
                    </div>

                    <div className="tc-settings-preview">
                      <div className="tc-settings-preview__title">
                        Will save as
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
                  </div>
                )}

                {showSharedFormFields ? (
                  <>
                    <div className="tc-settings-form__row" data-testid="settings-key-fields-row">
                      <KeySlotCombobox
                        feedback={keySlotRefreshFeedback}
                        hint="Search a configured key slot or type a new environment variable name."
                        onChange={handleKeySlotChange}
                        onRefresh={handleKeySlotRefresh}
                        options={keySlotOptions}
                        placeholder={suggestedApiKeyEnv || "EXAMPLE_API_KEY"}
                        refreshDisabled={!state.capabilities.listProviderKeys || isKeySlotRefreshing}
                        refreshLabel="Refresh key slots"
                        refreshing={isKeySlotRefreshing}
                        value={effectiveApiKeyEnv}
                      />
                      <label className="tc-field">
                        <div className="tc-field__label-row">
                          <span>{effectiveKeyPresent ? "New API key (optional)" : "API key"}</span>
                        </div>
                        <input
                          aria-label="API key"
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
                              ? `Leave blank to reuse ${effectiveApiKeyEnv}`
                              : `Save ${effectiveApiKeyEnv || "the selected key slot"}`
                          }
                          readOnly={!isApiKeyFocused && draftApiKey.length > 0}
                          type={isApiKeyFocused || !draftApiKey ? "password" : "text"}
                          value={
                            isApiKeyFocused || !draftApiKey
                              ? draftApiKey
                              : maskDraftApiKey(draftApiKey)
                          }
                        />
                        <small className="tc-field__hint">
                          {effectiveKeyPresent
                            ? `Already configured: ${effectiveApiKeyEnv}.`
                            : "Required unless you choose a configured slot."}
                        </small>
                      </label>
                    </div>

                    <button
                      aria-expanded={showAdvanced}
                      className="tc-settings-advanced__toggle"
                      onClick={() => setShowAdvanced((current) => !current)}
                      type="button"
                    >
                      <span>Advanced</span>
                      <span className="tc-settings-advanced__caret">
                        {showAdvanced ? "▾" : "▸"}
                      </span>
                    </button>

                    {showAdvanced ? (
                      <div className="tc-settings-advanced">
                        {dialogKind === "official" ? (
                          <div className="tc-settings-form__row">
                            <label className="tc-field">
                              <span>API override</span>
                              <select
                                aria-label="API override"
                                onChange={(event) => handleApiChange(event.target.value)}
                                value={fieldText(form.api) || selectedPreset?.api || ""}
                              >
                                {API_OPTIONS.map((entry) => (
                                  <option key={entry.value} value={entry.value}>
                                    {entry.label}
                                  </option>
                                ))}
                              </select>
                            </label>
                            <label className="tc-field">
                              <span>Base URL override</span>
                              <input
                                className="tc-input"
                                onChange={(event) =>
                                  setForm((current) => ({
                                    ...current,
                                    baseUrl: event.target.value,
                                  }))
                                }
                                placeholder={selectedPreset?.baseUrl || "https://api.example.com/v1"}
                                value={form.baseUrl ?? ""}
                              />
                            </label>
                          </div>
                        ) : null}

                        <div className="tc-settings-form__row">
                          <label className="tc-field">
                            <span>Model ID (alias)</span>
                            <input
                              className="tc-input"
                              disabled={selectedModel !== null}
                              onChange={(event) =>
                                setForm((current) => ({
                                  ...current,
                                  id: event.target.value,
                                }))
                              }
                              placeholder={derivedId || "Defaults to the model name"}
                              value={selectedModel ? selectedModel.id : form.id}
                            />
                            <small className="tc-field__hint">
                              Leave this empty to use the suggested alias.
                            </small>
                          </label>
                          <label className="tc-field">
                            <span>Provider override</span>
                            <input
                              className="tc-input"
                              onChange={(event) =>
                                setForm((current) => ({
                                  ...current,
                                  provider: event.target.value,
                                }))
                              }
                              placeholder={effectiveProvider || "Derived from the current mode"}
                              value={form.provider}
                            />
                            <small className="tc-field__hint">
                              Usually you should keep the derived provider label.
                            </small>
                          </label>
                        </div>

                        <div className="tc-settings-form__row">
                          <label className="tc-field">
                            <span>Context window</span>
                            <input
                              className="tc-input"
                              inputMode="numeric"
                              onChange={(event) =>
                                setForm((current) => ({
                                  ...current,
                                  contextWindow: event.target.value
                                    ? Number(event.target.value)
                                    : null,
                                }))
                              }
                              placeholder="400000"
                              value={normalizedContextWindow ?? ""}
                            />
                            <small className="tc-field__hint">
                              Leave this empty to use Tomcat’s default context window.
                            </small>
                          </label>
                        </div>

                        <label className="tc-field">
                          <span>Thinking format</span>
                          <select
                            aria-label="Thinking format"
                            onChange={(event) =>
                              setForm((current) => ({
                                ...current,
                                thinkingFormat:
                                  dialogKind === "official" &&
                                  selectedPreset &&
                                  event.target.value === selectedPreset.thinkingFormat
                                    ? ""
                                    : event.target.value,
                              }))
                            }
                            value={
                              fieldText(form.thinkingFormat) ||
                              (dialogKind === "official"
                                ? selectedPreset?.thinkingFormat ?? defaultThinkingFormatForApi(effectiveApi)
                                : defaultThinkingFormatForApi(effectiveApi))
                            }
                          >
                            {THINKING_FORMAT_OPTIONS.map((entry) => (
                              <option key={entry.value || "auto"} value={entry.value}>
                                {entry.label}
                              </option>
                            ))}
                          </select>
                          <small className="tc-field__hint">
                            Defaults follow the selected API. Override only if
                            your relay intentionally expects a different wire shape.
                          </small>
                        </label>

                        <div className="tc-settings-capabilities">
                          {CAPABILITY_OPTIONS.map(([key, label]) => (
                            <label className="tc-settings-capabilities__item" key={key}>
                              <input
                                checked={form.capabilities[key]}
                                onChange={(event) =>
                                  handleCapabilityChange(key, event.target.checked)
                                }
                                type="checkbox"
                              />
                              <span>{label}</span>
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
                    onClick={closeForm}
                    type="button"
                  >
                    Cancel
                  </button>
                  {selectedModel?.source === "user" ? (
                    <button
                      className="tc-button tc-button--ghost"
                      disabled={!state.capabilities.removeModel}
                      onClick={handleDelete}
                      type="button"
                    >
                      Delete
                    </button>
                  ) : null}
                  <button
                    className="tc-button tc-button--primary"
                    disabled={saveDisabled}
                    onClick={handleSave}
                    type="button"
                  >
                    Save Model
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
                  <h3 id="replace-shared-key-title">Replace shared API key?</h3>
                  <p>
                    You are about to replace <strong>{replacementConfirmation.envName}</strong>.
                  </p>
                </div>
              </div>
              <div className="tc-settings-form">
                <p>The following models will use the new key immediately:</p>
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
                    Cancel
                  </button>
                  <button
                    className="tc-button tc-button--primary"
                    onClick={() => {
                      setReplacementConfirmation(null);
                      submitModelSave();
                    }}
                    type="button"
                  >
                    Replace shared key
                  </button>
                </div>
              </div>
            </section>
          </div>
        ) : null}
      </main>
    </div>
  );
}
