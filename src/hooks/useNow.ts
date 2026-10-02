import { useEffect, useState } from "react";

const REFRESH_MS = 60_000;

/** Current time, refreshed each minute so day labels roll over at midnight. */
export function useNow(): Date {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const id = setInterval(() => setNow(new Date()), REFRESH_MS);
    return () => clearInterval(id);
  }, []);
  return now;
}
