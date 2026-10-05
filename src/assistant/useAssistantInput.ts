import { useRef, useState } from "react";
import { commands, type AssistantStateEvent } from "@/bindings";

/** Local-only draft; command acceptance must not erase edits made in flight. */
export function useAssistantInput(state: AssistantStateEvent | null) {
  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);
  const [errorKey, setErrorKey] = useState<string | null>(null);
  const revision = useRef(0);
  const pending = useRef(false);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const canSend = Boolean(
    state?.providerReady &&
      state.phase !== "thinking" &&
      !state.dictating &&
      draft.trim() &&
      !sending,
  );
  const change = (value: string) => {
    revision.current += 1;
    setDraft(value);
  };
  const reset = (version: number) => {
    if (revision.current === version) change("");
    setErrorKey(null);
    inputRef.current?.focus();
  };
  const send = async () => {
    if (!canSend || pending.current) return;
    const version = revision.current;
    pending.current = true;
    setSending(true);
    setErrorKey(null);
    try {
      const result = await commands.assistantSend(draft.trim());
      if (result.status === "error") setErrorKey("assistant.actionError.send");
      else if (revision.current === version) change("");
    } catch {
      setErrorKey("assistant.actionError.send");
    } finally {
      pending.current = false;
      setSending(false);
    }
  };
  return {
    draft,
    inputRef,
    change,
    reset,
    getRevision: () => revision.current,
    send,
    sending,
    canSend,
    errorKey,
  };
}
