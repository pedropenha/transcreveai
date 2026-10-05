import { useEffect, useRef, useState } from "react";

/** True once the element has been on screen (rows far below never ask). */
export function useSeen<T extends HTMLElement>(): [
  React.RefObject<T>,
  boolean,
] {
  const ref = useRef<T>(null);
  const [seen, setSeen] = useState(
    () => typeof IntersectionObserver === "undefined",
  );
  useEffect(() => {
    const node = ref.current;
    if (seen || !node) return;
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) {
        setSeen(true);
        observer.disconnect();
      }
    });
    observer.observe(node);
    return () => observer.disconnect();
  }, [seen]);
  return [ref, seen];
}
