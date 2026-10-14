import { useT } from "../i18n/LocaleProvider";

/** Readiness is owned by serve; the card only points to Settings, never to a particular model. */
export function ModelSetupCard({ onConfigure }: { onConfigure: () => void }) {
  const t = useT();
  return (
    <section className="tc-empty-state tc-model-setup" data-testid="model-setup-card" role="status">
      <h2>{t("modelSetup.title")}</h2>
      <button type="button" className="tc-button tc-button--primary" data-testid="configure-model" onClick={onConfigure}>
        {t("modelSetup.configure")}
      </button>
    </section>
  );
}
