import type { TomcatMessenger } from "../serveClient/TomcatMessenger";
import type { InitializeResult } from "../serveClient/initialize";
import { parseProjectTrustPayload } from "../shared/connectorsProtocol";
import { t } from "../shared/i18n";

/** The host asks on Serve readiness, even when Settings has never been opened. */
export class ProjectTrustPrompt {
  private readonly asked = new Set<string>();
  private readonly listeners = new Set<() => void>();
  private inFlight = false;

  constructor(private readonly deps: {
    getDefaultCwd(): string | undefined;
    messenger: Pick<TomcatMessenger, "sendGetProjectTrust" | "sendTrustProject">;
    confirm(root: string): Promise<boolean>;
    reportError(error: unknown, visible: boolean): void;
  }) {}

  onDidTrustProject(listener: () => void): { dispose(): void } {
    this.listeners.add(listener);
    return { dispose: () => { this.listeners.delete(listener); } };
  }

  async onServeReady(result: Pick<InitializeResult, "capabilities">): Promise<void> {
    if (this.inFlight || !result.capabilities.includes("get_project_trust")
      || !result.capabilities.includes("trust_project")) return;
    const cwd = this.deps.getDefaultCwd();
    if (!cwd) return;
    this.inFlight = true;
    let trustAttempted = false;
    try {
      const lookup = await this.deps.messenger.sendGetProjectTrust(cwd);
      if (!lookup.success) throw new Error(lookup.error ?? t("host.trustReadFailed"));
      const status = parseProjectTrustPayload(lookup.payload);
      if (status.error) throw new Error(status.error);
      if (status.trusted || this.asked.has(status.projectRoot)) return;
      this.asked.add(status.projectRoot);
      if (!await this.deps.confirm(status.projectRoot)) return;

      // A modal may outlive a workspace switch; compare the current project with
      // the path displayed in the modal before persisting anything.
      const currentCwd = this.deps.getDefaultCwd();
      if (!currentCwd) return;
      const current = await this.deps.messenger.sendGetProjectTrust(currentCwd);
      if (!current.success) throw new Error(current.error ?? t("host.trustVerifyFailed"));
      const verified = parseProjectTrustPayload(current.payload);
      if (verified.error || verified.projectRoot !== status.projectRoot) return;
      trustAttempted = true;
      const granted = await this.deps.messenger.sendTrustProject(status.projectRoot);
      if (!granted.success) throw new Error(granted.error ?? t("host.trustGrantFailed"));
      const receipt = parseProjectTrustPayload(granted.payload);
      if (!receipt.trusted || receipt.projectRoot !== status.projectRoot || receipt.error) {
        throw new Error(t("host.trustReceiptMismatch"));
      }
      for (const listener of this.listeners) listener();
    } catch (error) {
      this.deps.reportError(error, trustAttempted);
    } finally {
      this.inFlight = false;
    }
  }
}
