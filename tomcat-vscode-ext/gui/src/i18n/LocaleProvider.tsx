import { createContext, useContext, useEffect, useMemo, useSyncExternalStore, type ReactNode } from "react";
import { getLocale, subscribeLocale, translate, type Locale, type Translator } from "../../../src/shared/i18n";

const LocaleContext = createContext<Locale | null>(null);
export function LocaleProvider({ locale, children }: { locale?: Locale; children: ReactNode }) {
  const observed = useSyncExternalStore(subscribeLocale, getLocale, getLocale);
  const effective = locale ?? observed;
  useEffect(() => { document.documentElement.lang = effective; }, [effective]);
  return <LocaleContext.Provider value={effective}>{children}</LocaleContext.Provider>;
}
export function useLocale(): Locale {
  const context = useContext(LocaleContext);
  const observed = useSyncExternalStore(subscribeLocale, getLocale, getLocale);
  return context ?? observed;
}
export function useT(): Translator {
  const locale = useLocale();
  return useMemo(() => (key, args) => translate(locale, key, args), [locale]);
}
