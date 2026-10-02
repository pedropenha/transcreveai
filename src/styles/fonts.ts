/**
 * Self-hosted fonts ("Papel & Anil", ADR-0003).
 *
 * Imported once by every webview entrypoint (Hub, Flow Bar, toast, assistant,
 * meeting). Vite bundles the woff2 files and serves them from the app origin,
 * so typography works offline and never touches a font CDN.
 * - Instrument Sans (variable, weight 400-700): interface text.
 * - Instrument Serif (regular + italic): highlights only (banner, stat
 *   numbers, empty states) — never body text or controls.
 */
import "@fontsource-variable/instrument-sans/wght.css";
import "@fontsource/instrument-serif/400.css";
import "@fontsource/instrument-serif/400-italic.css";
