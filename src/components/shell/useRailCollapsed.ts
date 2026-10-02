import { useCallback, useState } from "react";

const STORAGE_KEY = "tai-rail-collapsed";

function readStored(): boolean {
  try {
    return localStorage.getItem(STORAGE_KEY) === "1";
  } catch {
    return false;
  }
}

/** Rail collapse state — a per-viewer convenience kept in localStorage. */
export function useRailCollapsed(): [boolean, () => void] {
  const [collapsed, setCollapsed] = useState(readStored);
  const toggle = useCallback(() => {
    setCollapsed((current) => {
      const next = !current;
      try {
        localStorage.setItem(STORAGE_KEY, next ? "1" : "0");
      } catch {
        /* storage unavailable: the toggle still works for this session */
      }
      return next;
    });
  }, []);
  return [collapsed, toggle];
}
