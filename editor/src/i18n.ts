import { ref, computed } from "vue";
import { zhCN, type LocaleMessages } from "./locales/zh-CN";
import { enUS } from "./locales/en-US";

export type SupportedLocale = "zh-CN" | "en-US";

const STORAGE_KEY = "prisma-editor.locale.v1";

function detectInitialLocale(): SupportedLocale {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (saved === "zh-CN" || saved === "en-US") return saved;
  } catch {
    // Local storage unavailable
  }
  if (typeof navigator !== "undefined" && navigator.language?.startsWith("en")) {
    return "en-US";
  }
  return "zh-CN";
}

export const currentLocale = ref<SupportedLocale>(detectInitialLocale());

const messages: Record<SupportedLocale, LocaleMessages> = {
  "zh-CN": zhCN,
  "en-US": enUS,
};

export function setLocale(locale: SupportedLocale) {
  currentLocale.value = locale;
  try {
    localStorage.setItem(STORAGE_KEY, locale);
  } catch {
    // Local storage unavailable
  }
}

/**
 * Translate a dot-separated key path with optional replacement parameters.
 * E.g.: `t("inspector.selectedCount", { count: 3 })`
 */
export function t(path: string, params?: Record<string, string | number>): string {
  const dict = messages[currentLocale.value] ?? zhCN;
  const fallbackDict = messages["en-US"] ?? enUS;

  let value: unknown = dict;
  for (const part of path.split(".")) {
    if (value && typeof value === "object" && part in value) {
      value = (value as Record<string, unknown>)[part];
    } else {
      value = undefined;
      break;
    }
  }

  if (typeof value !== "string") {
    // Fallback to English dictionary
    let fallbackValue: unknown = fallbackDict;
    for (const part of path.split(".")) {
      if (fallbackValue && typeof fallbackValue === "object" && part in fallbackValue) {
        fallbackValue = (fallbackValue as Record<string, unknown>)[part];
      } else {
        fallbackValue = undefined;
        break;
      }
    }
    value = typeof fallbackValue === "string" ? fallbackValue : path;
  }

  let text = String(value);
  if (params) {
    for (const [k, v] of Object.entries(params)) {
      text = text.split(`{${k}}`).join(String(v));
    }
  }
  return text;
}

export const isZh = computed(() => currentLocale.value === "zh-CN");

export function useI18n() {
  return {
    locale: currentLocale,
    setLocale,
    t,
    isZh,
  };
}

export function createI18nPlugin() {
  return {
    install(app: any) {
      app.config.globalProperties.$t = t;
      app.provide("i18n", {
        locale: currentLocale,
        setLocale,
        t,
      });
    },
  };
}

declare module "vue" {
  interface ComponentCustomProperties {
    $t: typeof t;
  }
}
