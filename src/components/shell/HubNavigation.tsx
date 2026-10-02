import { createContext, useContext } from "react";
import type { Navigation } from "./navModel";

type NavigateFn = (navigation: Navigation) => void;

const noop: NavigateFn = () => {};

/**
 * Lets screens rendered by the Hub shell (which take no props) move to another
 * section or Settings tab — e.g. the banner's "Choose a model" button.
 */
export const HubNavigationContext = createContext<NavigateFn>(noop);

export function useHubNavigation(): NavigateFn {
  return useContext(HubNavigationContext);
}
