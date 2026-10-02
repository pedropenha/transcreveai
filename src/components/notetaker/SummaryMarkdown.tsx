import React from "react";
import ReactMarkdown from "react-markdown";

const ALLOWED = [
  "p",
  "ul",
  "ol",
  "li",
  "strong",
  "em",
  "code",
  "br",
  "blockquote",
  "a",
];

/**
 * Summary body. Raw HTML is never rendered (react-markdown escapes it) and
 * links are shown as plain text: a model-written summary must not be able to
 * navigate the Hub webview.
 */
export const SummaryMarkdown: React.FC<{ markdown: string }> = ({
  markdown,
}) => (
  <div className="nt-md">
    <ReactMarkdown
      allowedElements={ALLOWED}
      unwrapDisallowed
      components={{ a: ({ children }) => <span>{children}</span> }}
    >
      {markdown}
    </ReactMarkdown>
  </div>
);
