import { en, ko } from "./catalog";
import { browserLanguages, createTranslator, format, formatParts, placeholders, resolveLocale, setLocale, systemLocale, type Message } from "./core";
import { loadLocalePreference, saveLocalePreference, t } from "./index";

function sorted(message: Message): string[] {
  return [...placeholders(message)].sort();
}

describe("catalog parity", () => {
  const enKeys = Object.keys(en).sort();
  const koKeys = Object.keys(ko).sort();

  it("has every key in both languages", () => {
    expect(koKeys.filter((key) => !(key in en))).toEqual([]);
    expect(enKeys.filter((key) => !(key in ko))).toEqual([]);
  });

  it("uses the same placeholders in both languages", () => {
    const mismatched = enKeys.filter((key) => sorted(en[key as keyof typeof en]).join() !== sorted(ko[key as keyof typeof ko]).join());
    expect(mismatched).toEqual([]);
  });

  it("has no empty Korean values", () => {
    const empty = koKeys.filter((key) => {
      const message: Message = ko[key as keyof typeof ko];
      return (typeof message === "string" ? [message] : Object.values(message)).some((form) => form.trim() === "");
    });
    expect(empty).toEqual([]);
  });
});

describe("i18n core", () => {
  afterEach(() => setLocale("en"));

  it("resolves the system language", () => {
    expect(systemLocale(["ko-KR", "en-US"])).toBe("ko");
    expect(systemLocale(["fr-FR", "ko"])).toBe("ko");
    expect(systemLocale(["fr-FR"])).toBe("en");
    expect(systemLocale([])).toBe("en");
    expect(resolveLocale("en", ["ko-KR"])).toBe("en");
  });

  it("falls back to navigator.language when the language list is empty", () => {
    expect(browserLanguages({ languages: [], language: "ko-KR" })).toEqual(["ko-KR"]);
    expect(browserLanguages({ languages: ["en-US", "ko"], language: "en-US" })).toEqual(["en-US", "ko"]);
    expect(browserLanguages({ languages: [], language: "" })).toEqual([]);
    expect(systemLocale(browserLanguages({ languages: [], language: "ko-KR" }))).toBe("ko");
  });

  it("interpolates and picks plural forms", () => {
    const plural = { one: "{count} task", other: "{count} tasks" };
    expect(format(plural, { count: 1 }, "en")).toBe("1 task");
    expect(format(plural, { count: 1200 }, "en")).toBe("1,200 tasks");
    expect(format({ other: "작업 {count}개" }, { count: 3 }, "ko")).toBe("작업 3개");
    expect(format("Hi {name}, {missing}", { name: "Ada" }, "en")).toBe("Hi Ada, {missing}");
    expect(formatParts("Type {key} now", { key: { el: 1 } }, "en")).toEqual(["Type ", { el: 1 }, " now"]);
  });

  it("follows the active locale and falls back to English", () => {
    const translate = createTranslator({ en: { greet: "Hello", only: "English only" }, ko: { greet: "안녕하세요", only: "" } });
    setLocale("ko");
    expect(translate("greet")).toBe("안녕하세요");
    setLocale("en");
    expect(translate("greet")).toBe("Hello");
  });

  it("stores the preference and sets the document language", () => {
    saveLocalePreference("ko");
    expect(loadLocalePreference()).toBe("ko");
    expect(document.documentElement.lang).toBe("ko");
    expect(t("common.cancel")).toBe(ko["common.cancel"]);
    saveLocalePreference("system");
    expect(loadLocalePreference()).toBe("system");
    expect(document.documentElement.lang).toBe("en");
  });
});
