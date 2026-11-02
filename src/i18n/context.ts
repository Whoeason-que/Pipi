import {
  createElement,
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import {
  getDefaultLocale,
  getTranslation,
  STORAGE_LOCALE_KEY,
  type Locale,
} from "./core.ts";
import { writeStored } from "../storage.ts";

export interface I18nContextValue {
  locale: Locale;
  setLocale: (next: Locale) => void;
  t: (path: string, params?: Record<string, string | number>) => string;
}

export const I18nContext = createContext<I18nContextValue>({
  locale: "en",
  setLocale: () => {},
  t: (path, params) => getTranslation("en", path, params),
});

export function I18nProvider({ children }: { children: ReactNode }) {
  const [locale, setLocaleState] = useState<Locale>(getDefaultLocale);

  const setLocale = useCallback((next: Locale) => {
    setLocaleState(next);
    writeStored(STORAGE_LOCALE_KEY, next);
    if (typeof document !== "undefined") {
      document.documentElement.lang = next;
    }
  }, []);

  useEffect(() => {
    if (typeof document !== "undefined") {
      document.documentElement.lang = locale;
    }
  }, [locale]);

  const t = useCallback(
    (path: string, params?: Record<string, string | number>) => {
      return getTranslation(locale, path, params);
    },
    [locale],
  );

  const value = useMemo(
    () => ({ locale, setLocale, t }),
    [locale, setLocale, t],
  );

  return createElement(I18nContext.Provider, { value }, children);
}

export function useI18n() {
  return useContext(I18nContext);
}
