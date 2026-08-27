import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { en, type TranslationKey } from "./en";
import { ja } from "./ja";
import { zh } from "./zh-CN";

export type Locale = "en" | "zh-CN" | "ja";
type Params = Record<string, string | number>;
export type { TranslationKey };

const messages: Record<Locale, Record<TranslationKey, string>> = {
  en,
  "zh-CN": zh,
  ja,
};

export const localeOptions: Array<{ value: Locale; labelKey: TranslationKey; short: string }> = [
  { value: "en", labelKey: "language.english", short: "EN" },
  { value: "zh-CN", labelKey: "language.chinese", short: "中" },
  { value: "ja", labelKey: "language.japanese", short: "日" },
];

function detectLocale(): Locale {
  const stored = window.localStorage.getItem("codex-hub-locale");
  if (stored === "en" || stored === "zh-CN" || stored === "ja") return stored;
  const languages = navigator.languages?.length ? navigator.languages : [navigator.language];
  for (const language of languages) {
    const normalized = language.toLowerCase();
    if (normalized.startsWith("zh")) return "zh-CN";
    if (normalized.startsWith("ja")) return "ja";
  }
  return "en";
}

function interpolate(value: string, params?: Params): string {
  if (!params) return value;
  return value.replace(/\{(\w+)\}/g, (_, key: string) => String(params[key] ?? `{${key}}`));
}

export function localeTag(locale: Locale): string {
  if (locale === "zh-CN") return "zh-CN";
  if (locale === "ja") return "ja-JP";
  return "en-US";
}

interface I18nValue {
  locale: Locale;
  setLocale: (locale: Locale) => void;
  t: (key: TranslationKey, params?: Params) => string;
}

const I18nContext = createContext<I18nValue | null>(null);

export function I18nProvider({ children }: { children: ReactNode }) {
  const [locale, setLocale] = useState<Locale>(detectLocale);

  useEffect(() => {
    window.localStorage.setItem("codex-hub-locale", locale);
    document.documentElement.lang = locale;
  }, [locale]);

  const value = useMemo<I18nValue>(
    () => ({
      locale,
      setLocale,
      t: (key, params) => interpolate(messages[locale][key] ?? en[key], params),
    }),
    [locale],
  );

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

export function useI18n(): I18nValue {
  const value = useContext(I18nContext);
  if (!value) throw new Error("useI18n must be used inside I18nProvider");
  return value;
}

export function formatRelativeTime(
  iso: string | null | undefined,
  locale: Locale,
  t: I18nValue["t"],
): string {
  if (!iso) return "—";
  const timestamp = Date.parse(iso);
  if (Number.isNaN(timestamp)) return iso;
  const seconds = Math.max(0, Math.round((Date.now() - timestamp) / 1000));
  if (seconds < 5) return t("common.justNow");
  if (seconds < 60) return t("common.secondsAgo", { count: seconds });
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return t("common.minutesAgo", { count: minutes });
  return new Date(timestamp).toLocaleTimeString(localeTag(locale), {
    hour: "2-digit",
    minute: "2-digit",
  });
}
