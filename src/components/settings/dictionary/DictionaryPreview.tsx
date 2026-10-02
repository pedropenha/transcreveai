import React, { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { highlightTerms, previewCrutchRemoval } from "./dictionaryView";

interface DictionaryPreviewProps {
  vocabulary: readonly string[];
  /** Crutches to remove in the preview (empty when cleanup is off). */
  crutches: readonly string[];
}

/**
 * "Como o dicionário age": an editable sample, the text as spoken and the
 * approximate result. Vocabulary terms found in the sample are highlighted
 * (they bias the model); crutches are removed using the user's own list.
 */
export const DictionaryPreview: React.FC<DictionaryPreviewProps> = ({
  vocabulary,
  crutches,
}) => {
  const { t } = useTranslation();
  const [sample, setSample] = useState(() => t("dictionary.preview.sample"));
  const inputId = useId();
  const after = previewCrutchRemoval(sample, crutches);

  return (
    <aside className="dict-explain" aria-labelledby={`${inputId}-title`}>
      <h2 id={`${inputId}-title`}>
        {t("dictionary.preview.titlePre")}
        <em>{t("dictionary.preview.titleEm")}</em>
      </h2>
      <p>{t("dictionary.preview.body")}</p>

      <label htmlFor={inputId} className="caps">
        {t("dictionary.preview.said")}
      </label>
      <textarea
        id={inputId}
        value={sample}
        rows={3}
        maxLength={400}
        onChange={(event) => setSample(event.target.value)}
      />

      <div className="dict-ba" data-testid="dictionary-after">
        <span className="caps">{t("dictionary.preview.written")}</span>
        <p>
          {highlightTerms(after, vocabulary).map((segment, index) =>
            segment.hit ? (
              <mark key={index}>{segment.text}</mark>
            ) : (
              <React.Fragment key={index}>{segment.text}</React.Fragment>
            ),
          )}
        </p>
      </div>
      <p className="dict-note">{t("dictionary.preview.note")}</p>
    </aside>
  );
};
