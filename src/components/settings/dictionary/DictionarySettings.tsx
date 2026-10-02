import React, { useEffect, useRef, useState } from "react";
import { Plus, RotateCcw, Search, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { commands } from "@/bindings";
import { useSettings } from "@/hooks/useSettings";
import { useSettingsStore } from "@/stores/settingsStore";
import { DictionaryPreview } from "./DictionaryPreview";
import { MAX_TERM_LENGTH, filterTerms, normalizeTerm } from "./dictionaryView";
import "./dictionary.css";

type Tab = "vocabulary" | "replacements" | "crutches";
const TABS: readonly Tab[] = ["vocabulary", "replacements", "crutches"];

/**
 * Dicionário (F004 stage 4, FR-010-28): vocabulary (hints for the model),
 * replacements (v1.1+, FR-004-09) and crutches (words the `light` cleanup
 * removes), with a before/after preview of what the lists do.
 */
export const DictionarySettings: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const [tab, setTab] = useState<Tab>("vocabulary");
  const [draft, setDraft] = useState("");
  const [query, setQuery] = useState("");
  const [fillers, setFillers] = useState<string[]>([]);
  const [loadingFillers, setLoadingFillers] = useState(true);
  const [fillerLoadFailed, setFillerLoadFailed] = useState(false);
  const [loadVersion, setLoadVersion] = useState(0);
  const [saving, setSaving] = useState(false);
  const writing = useRef(false);
  const addRef = useRef<HTMLInputElement>(null);

  const words = getSetting("custom_words") ?? [];
  const cleanupEnabled = getSetting("filler_word_removal_enabled") ?? true;

  useEffect(() => {
    let cancelled = false;
    setLoadingFillers(true);
    setFillerLoadFailed(false);
    void commands
      .getFillerWords()
      .then((result) => {
        if (cancelled) return;
        if (result.status === "ok") setFillers(result.data);
        else setFillerLoadFailed(true);
      })
      .catch(() => {
        if (!cancelled) setFillerLoadFailed(true);
      })
      .finally(() => {
        if (!cancelled) setLoadingFillers(false);
      });
    return () => {
      cancelled = true;
    };
  }, [loadVersion]);

  const switchTab = (next: Tab) => {
    if (writing.current) return;
    setTab(next);
    setDraft("");
    setQuery("");
  };

  const persistFillers = async (next: string[]) => {
    try {
      const result = await commands.setFillerWords(next);
      if (result.status === "ok") {
        setFillers(next);
        return true;
      }
    } catch (e) {
      console.warn("set_filler_words failed:", e);
    }
    toast.error(t("dictionary.fillers.saveError"));
    return false;
  };

  const persistWords = async (next: string[]) => {
    try {
      const result = await commands.updateCustomWords(next);
      if (result.status === "ok") {
        useSettingsStore.setState((state) => ({
          settings: state.settings
            ? { ...state.settings, custom_words: next }
            : null,
        }));
        return true;
      }
    } catch (e) {
      console.warn("update_custom_words failed:", e);
    }
    toast.error(t("dictionary.saveError"));
    return false;
  };

  const runWrite = async (operation: () => Promise<void>) => {
    if (writing.current) return;
    writing.current = true;
    setSaving(true);
    try {
      await operation();
    } finally {
      writing.current = false;
      setSaving(false);
    }
  };

  const resetFillers = async () => {
    try {
      const reset = await commands.resetFillerWords();
      if (reset.status === "ok") {
        const result = await commands.getFillerWords();
        if (result.status === "ok") {
          setFillers(result.data);
          return;
        }
      }
    } catch (e) {
      console.warn("reset_filler_words failed:", e);
    }
    toast.error(t("dictionary.fillers.saveError"));
  };

  const addTerm = async () => {
    const value = normalizeTerm(draft);
    if (!value || value.length > MAX_TERM_LENGTH) return;
    if (tab === "vocabulary") {
      if (words.includes(value)) {
        toast.error(t("dictionary.duplicate", { term: value }));
        return;
      }
      if (!(await persistWords([...words, value]))) return;
    } else if (tab === "crutches") {
      const lowered = value.toLocaleLowerCase();
      if (fillers.includes(lowered)) {
        toast.error(t("dictionary.duplicate", { term: lowered }));
        return;
      }
      if (!(await persistFillers([...fillers, lowered]))) return;
    }
    setDraft("");
  };

  const removeTerm = (term: string) =>
    void runWrite(async () => {
      if (tab === "vocabulary")
        await persistWords(words.filter((value) => value !== term));
      else await persistFillers(fillers.filter((value) => value !== term));
    });

  const terms = tab === "vocabulary" ? words : fillers;
  const visible = filterTerms(terms, query);
  const counts: Record<Tab, number | null> = {
    vocabulary: words.length,
    replacements: null,
    crutches: fillers.length,
  };
  const editable =
    tab !== "replacements" &&
    !saving &&
    !(tab === "crutches" && (loadingFillers || fillerLoadFailed));

  return (
    <main className="dict-page">
      <header className="dict-head">
        <h1>{t("dictionary.title")}</h1>
        <button
          type="button"
          className="dict-primary"
          disabled={!editable}
          onClick={() => addRef.current?.focus()}
        >
          <Plus width={15} height={15} aria-hidden="true" />
          {t("dictionary.addWord")}
        </button>
      </header>
      <p className="dict-sub">{t("dictionary.subtitle")}</p>

      <div className="dict-grid">
        <div className="dict-main">
          <div
            className="dict-seg"
            role="group"
            aria-label={t("dictionary.tabs.label")}
          >
            {TABS.map((value) => (
              <button
                key={value}
                type="button"
                disabled={saving}
                aria-pressed={tab === value}
                onClick={() => switchTab(value)}
              >
                {t(`dictionary.tabs.${value}`)}
                {counts[value] !== null ? (
                  <span className="dict-count">{counts[value]}</span>
                ) : (
                  <span className="dict-soon">{t("settingsHub.soon")}</span>
                )}
              </button>
            ))}
          </div>

          {tab === "replacements" ? (
            <div className="dict-empty" data-testid="dictionary-replacements">
              <h2>{t("dictionary.replacements.title")}</h2>
              <p>{t("dictionary.replacements.body")}</p>
              <div className="dict-row dict-row-ghost" aria-hidden="true">
                <span className="dict-term">
                  <s>{t("dictionary.replacements.exampleFrom")}</s>
                  <span className="dict-arrow">→</span>
                  {t("dictionary.replacements.exampleTo")}
                </span>
              </div>
            </div>
          ) : (
            <>
              {tab === "crutches" ? (
                <label className="dict-switch">
                  <input
                    type="checkbox"
                    role="switch"
                    checked={cleanupEnabled}
                    disabled={isUpdating("filler_word_removal_enabled")}
                    onChange={(event) =>
                      void updateSetting(
                        "filler_word_removal_enabled",
                        event.target.checked,
                      )
                    }
                  />
                  <span>
                    <b>{t("settings.advanced.fillerWordRemoval.title")}</b>
                    <small>{t("dictionary.fillers.description")}</small>
                  </span>
                </label>
              ) : (
                <p className="dict-hint">
                  {t("dictionary.vocabulary.description")}
                </p>
              )}

              <form
                className="dict-toolbar"
                onSubmit={(event) => {
                  event.preventDefault();
                  void runWrite(addTerm);
                }}
              >
                <label className="dict-field">
                  <span className="sr-only">
                    {t(
                      tab === "vocabulary"
                        ? "dictionary.vocabulary.inputLabel"
                        : "dictionary.fillers.inputLabel",
                    )}
                  </span>
                  <input
                    ref={addRef}
                    value={draft}
                    maxLength={MAX_TERM_LENGTH}
                    disabled={!editable}
                    placeholder={t(
                      tab === "vocabulary"
                        ? "dictionary.vocabulary.placeholder"
                        : "dictionary.fillers.placeholder",
                    )}
                    onChange={(event) => setDraft(event.target.value)}
                  />
                </label>
                <button
                  type="submit"
                  className="dict-secondary"
                  disabled={
                    !normalizeTerm(draft) ||
                    !editable ||
                    (tab === "vocabulary" && isUpdating("custom_words"))
                  }
                >
                  {t("dictionary.add")}
                </button>
                {tab === "crutches" ? (
                  <button
                    type="button"
                    className="dict-link"
                    disabled={!editable}
                    onClick={() => void runWrite(resetFillers)}
                  >
                    <RotateCcw width={13} height={13} aria-hidden="true" />
                    {t("dictionary.fillers.reset")}
                  </button>
                ) : null}
              </form>

              <label className="dict-field dict-search">
                <Search width={15} height={15} aria-hidden="true" />
                <span className="sr-only">{t("dictionary.search")}</span>
                <input
                  type="search"
                  value={query}
                  placeholder={t("dictionary.search")}
                  onChange={(event) => setQuery(event.target.value)}
                />
              </label>

              {tab === "crutches" && (loadingFillers || fillerLoadFailed) ? (
                <div className="dict-empty" role="status">
                  <p>
                    {t(
                      loadingFillers
                        ? "dictionary.fillers.loading"
                        : "dictionary.fillers.loadError",
                    )}
                  </p>
                  {fillerLoadFailed ? (
                    <button
                      type="button"
                      className="dict-secondary"
                      onClick={() => setLoadVersion((value) => value + 1)}
                    >
                      {t("dictionary.retry")}
                    </button>
                  ) : null}
                </div>
              ) : visible.length === 0 ? (
                <div className="dict-empty" data-testid="dictionary-empty">
                  <h2>
                    {t(
                      query
                        ? "dictionary.empty.noMatch"
                        : `dictionary.empty.${tab}.title`,
                    )}
                  </h2>
                  {query ? null : <p>{t(`dictionary.empty.${tab}.body`)}</p>}
                </div>
              ) : (
                <ul className="dict-list" aria-live="polite">
                  {visible.map((term) => (
                    <li key={term} className="dict-row">
                      <span className="dict-term">{term}</span>
                      {tab === "vocabulary" ? (
                        <span className="dict-tag">
                          {t("dictionary.yours")}
                        </span>
                      ) : null}
                      <button
                        type="button"
                        className="dict-icon"
                        disabled={!editable}
                        aria-label={t("dictionary.remove", { term })}
                        title={t("dictionary.remove", { term })}
                        onClick={() => removeTerm(term)}
                      >
                        <Trash2 width={15} height={15} aria-hidden="true" />
                      </button>
                    </li>
                  ))}
                </ul>
              )}
            </>
          )}
        </div>

        <DictionaryPreview
          vocabulary={words}
          crutches={cleanupEnabled ? fillers : []}
        />
      </div>
    </main>
  );
};
