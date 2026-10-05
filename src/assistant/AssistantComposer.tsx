import { ArrowUp, Mic, Square } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { RefObject } from "react";

interface Props {
  draft: string;
  inputRef: RefObject<HTMLTextAreaElement>;
  onChange: (value: string) => void;
  onSend: () => void;
  onDictate: () => void;
  canSend: boolean;
  canDictate: boolean;
  dictating: boolean;
}

export function AssistantComposer({
  draft,
  inputRef,
  onChange,
  onSend,
  onDictate,
  canSend,
  canDictate,
  dictating,
}: Props) {
  const { t } = useTranslation();
  return (
    <footer className="as-foot">
      <div className="as-compose">
        <textarea
          ref={inputRef}
          aria-label={t("assistant.inputLabel")}
          placeholder={t("assistant.inputPlaceholder")}
          value={draft}
          rows={2}
          onChange={(e) => onChange(e.target.value)}
          onKeyDown={(e) => {
            if (
              e.key === "Enter" &&
              !e.shiftKey &&
              !e.nativeEvent.isComposing &&
              e.keyCode !== 229
            ) {
              e.preventDefault();
              if (canSend) onSend();
            }
          }}
        />
        <div className="as-compose-actions">
          <button
            type="button"
            className={`as-voice${dictating ? " as-voice--active" : ""}`}
            disabled={!canDictate}
            onClick={onDictate}
          >
            {dictating ? (
              <Square size={16} aria-hidden="true" />
            ) : (
              <Mic size={16} aria-hidden="true" />
            )}
            {t(dictating ? "assistant.stopDictation" : "assistant.dictate")}
          </button>
          <button
            type="button"
            className="as-send"
            disabled={!canSend}
            onClick={onSend}
            aria-label={t("assistant.send")}
            title={t("assistant.send")}
          >
            <ArrowUp size={18} aria-hidden="true" />
          </button>
        </div>
      </div>
      <span className="as-hint">{t("assistant.composerHint")}</span>
    </footer>
  );
}
