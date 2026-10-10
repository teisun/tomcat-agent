import { useT } from "../i18n/LocaleProvider";

function compactPrompt(text: string): string {
  return text.replace(/\s+/g, " ").trim();
}

export function StickyUserPrompt({ text }: { text: string }) {
  const t = useT();
  const compactText = compactPrompt(text);
  if (!compactText) {
    return null;
  }

  return (
    <section
      className="tc-sticky-prompt"
      data-testid="sticky-user-prompt"
      title={compactText}
    >
      <div className="tc-sticky-prompt__label">{t("message.label.user")}</div>
      <div className="tc-sticky-prompt__text" data-testid="sticky-user-prompt-text">
        {compactText}
      </div>
    </section>
  );
}
