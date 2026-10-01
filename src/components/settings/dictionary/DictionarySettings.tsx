import React, { useEffect, useState } from "react";
import { RotateCcw, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { commands } from "@/bindings";
import { useSettings } from "@/hooks/useSettings";
import { Button } from "../../ui/Button";
import { Input } from "../../ui/Input";

function normalize(value: string): string {
  return value.replace(/\s+/g, " ").trim();
}

export const DictionarySettings: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const [word, setWord] = useState("");
  const [filler, setFiller] = useState("");
  const [fillers, setFillers] = useState<string[]>([]);
  const [loadingFillers, setLoadingFillers] = useState(true);
  const words = getSetting("custom_words") ?? [];

  useEffect(() => {
    void commands.getFillerWords().then((result) => {
      if (result.status === "ok") setFillers(result.data);
      setLoadingFillers(false);
    });
  }, []);

  const addWord = async () => {
    const value = normalize(word);
    if (!value || words.includes(value)) return;
    await updateSetting("custom_words", [...words, value]);
    setWord("");
  };

  const persistFillers = async (next: string[]) => {
    const result = await commands.setFillerWords(next);
    if (result.status === "ok") setFillers(next);
    else toast.error(t("dictionary.fillers.saveError"));
  };

  const addFiller = async () => {
    const value = normalize(filler).toLocaleLowerCase();
    if (!value || fillers.includes(value)) return;
    await persistFillers([...fillers, value]);
    setFiller("");
  };

  const resetFillers = async () => {
    const reset = await commands.resetFillerWords();
    if (reset.status !== "ok")
      return toast.error(t("dictionary.fillers.saveError"));
    const result = await commands.getFillerWords();
    if (result.status === "ok") setFillers(result.data);
  };

  return (
    <main className="dictionary-page">
      <header className="section-page-header">
        <h1>{t("dictionary.title")}</h1>
        <p>{t("dictionary.subtitle")}</p>
      </header>
      <section
        className="dictionary-section"
        aria-labelledby="dictionary-vocabulary-title"
      >
        <div>
          <h2 id="dictionary-vocabulary-title">
            {t("dictionary.vocabulary.title")}
          </h2>
          <p>{t("dictionary.vocabulary.description")}</p>
        </div>
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void addWord();
          }}
          className="dictionary-form"
        >
          <label htmlFor="dictionary-word" className="sr-only">
            {t("dictionary.vocabulary.inputLabel")}
          </label>
          <Input
            id="dictionary-word"
            value={word}
            onChange={(event) => setWord(event.target.value)}
            placeholder={t("dictionary.vocabulary.placeholder")}
          />
          <Button
            type="submit"
            disabled={!normalize(word) || isUpdating("custom_words")}
          >
            {t("dictionary.add")}
          </Button>
        </form>
        <ul className="dictionary-list" aria-live="polite">
          {words.map((item) => (
            <li key={item}>
              <span>{item}</span>
              <button
                type="button"
                onClick={() =>
                  void updateSetting(
                    "custom_words",
                    words.filter((value) => value !== item),
                  )
                }
                aria-label={t("dictionary.remove", { term: item })}
              >
                <X aria-hidden="true" />
              </button>
            </li>
          ))}
        </ul>
      </section>
      <section
        className="dictionary-section"
        aria-labelledby="dictionary-fillers-title"
      >
        <div className="dictionary-section-heading">
          <div>
            <h2 id="dictionary-fillers-title">
              {t("dictionary.fillers.title")}
            </h2>
            <p>{t("dictionary.fillers.description")}</p>
          </div>
          <button
            type="button"
            onClick={() => void resetFillers()}
            className="dictionary-reset"
          >
            <RotateCcw aria-hidden="true" />
            {t("dictionary.fillers.reset")}
          </button>
        </div>
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void addFiller();
          }}
          className="dictionary-form"
        >
          <label htmlFor="dictionary-filler" className="sr-only">
            {t("dictionary.fillers.inputLabel")}
          </label>
          <Input
            id="dictionary-filler"
            value={filler}
            onChange={(event) => setFiller(event.target.value)}
            placeholder={t("dictionary.fillers.placeholder")}
            disabled={loadingFillers}
          />
          <Button type="submit" disabled={!normalize(filler) || loadingFillers}>
            {t("dictionary.add")}
          </Button>
        </form>
        <ul className="dictionary-list" aria-live="polite">
          {fillers.map((item) => (
            <li key={item}>
              <span>{item}</span>
              <button
                type="button"
                onClick={() =>
                  void persistFillers(fillers.filter((value) => value !== item))
                }
                aria-label={t("dictionary.remove", { term: item })}
              >
                <X aria-hidden="true" />
              </button>
            </li>
          ))}
        </ul>
      </section>
    </main>
  );
};
