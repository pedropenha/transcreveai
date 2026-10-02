import { createAppIconResolver } from "./appIconCore";

export type { ResolvedAppIcon } from "./appIconCore";

/** Bundled logos (`src/assets/app-icons/<slug>.svg`). The glob tolerates an
 *  empty/absent folder — missing logos resolve to the fallback glyph. */
const bundled = import.meta.glob("../assets/app-icons/*.svg", {
  eager: true,
  query: "?url",
  import: "default",
}) as Record<string, string>;

const bundledBySlug: Record<string, string> = Object.fromEntries(
  Object.entries(bundled).map(([path, url]) => [
    path.replace(/^.*\/([^/]+)\.svg$/, "$1"),
    url,
  ]),
);

export const resolveAppIcon = createAppIconResolver(bundledBySlug);
