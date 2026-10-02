import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useState,
  useRef,
  type ReactNode,
} from "react";
import { toast, Toaster } from "sonner";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { platform } from "@tauri-apps/plugin-os";
import {
  checkAccessibilityPermission,
  checkMicrophonePermission,
} from "tauri-plugin-macos-permissions-api";
import { ModelStateEvent, RecordingErrorEvent } from "./lib/types/events";
import "./App.css";
import AccessibilityPermissions from "./components/AccessibilityPermissions";
import SecureInputWarning from "./components/SecureInputWarning";
import Footer from "./components/footer";
import Onboarding, { AccessibilityOnboarding } from "./components/onboarding";
import { type OnboardingPreviewStep } from "./components/settings";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { Sidebar, SidebarSection, SECTIONS_CONFIG } from "./components/Sidebar";
import { useRailCollapsed } from "./components/shell/useRailCollapsed";
import { HubNavigationContext } from "./components/shell/HubNavigation";
import {
  resolveNavigation,
  sectionForShortcut,
  type Navigation,
} from "./components/shell/navModel";
import { handleNavigatePayload } from "./components/settings/hub/pendingSettingsTab";
import { WhatsNewGate } from "./components/whats-new";
import MeetingConsentGate from "./components/MeetingConsentGate";
import { useSettings } from "./hooks/useSettings";
import { useSettingsStore } from "./stores/settingsStore";
import { commands } from "@/bindings";
import { getLanguageDirection, initializeRTL } from "@/lib/utils/rtl";

type OnboardingStep = "accessibility" | "model" | "done";

// Stable identity so preview effects do not re-run due to callback changes.
const NOOP = () => {};

const renderSettingsContent = (section: SidebarSection) => {
  const ActiveComponent =
    SECTIONS_CONFIG[section]?.component || SECTIONS_CONFIG.home.component;
  return <ActiveComponent />;
};

function App() {
  const { t, i18n } = useTranslation();
  const [onboardingStep, setOnboardingStep] = useState<OnboardingStep | null>(
    null,
  );
  const [onboardingPreview, setOnboardingPreview] =
    useState<OnboardingPreviewStep | null>(null);
  // Track if this is a returning user who just needs to grant permissions
  // (vs a new user who needs full onboarding including model selection)
  const [isReturningUser, setIsReturningUser] = useState(false);
  const [currentSection, setCurrentSection] = useState<SidebarSection>("home");
  const [railCollapsed, toggleRailCollapsed] = useRailCollapsed();
  const { settings, updateSetting } = useSettings();
  const direction = getLanguageDirection(i18n.language);
  // Home, Notetaker and Settings own their scrolling (nav / list / details
  // scroll apart from the page).
  const isFullBleed =
    currentSection === "home" ||
    currentSection === "meetings" ||
    currentSection === "dictionary" ||
    currentSection === "settings";
  const refreshAudioDevices = useSettingsStore(
    (state) => state.refreshAudioDevices,
  );
  const refreshOutputDevices = useSettingsStore(
    (state) => state.refreshOutputDevices,
  );
  const hasCompletedPostOnboardingInit = useRef(false);
  const settingsScrollRef = useRef<HTMLDivElement>(null);
  const isShowingOnboarding =
    onboardingPreview !== null ||
    onboardingStep === "accessibility" ||
    onboardingStep === "model";

  // Classic scrollbars consume layout space. Reserve a matching gutter on the
  // opposite edge while onboarding is visible so its content stays centered in
  // the physical window. Overlay scrollbars ignore scrollbar-gutter.
  useLayoutEffect(() => {
    const attribute = "data-onboarding-active";
    document.documentElement.toggleAttribute(attribute, isShowingOnboarding);
    return () => document.documentElement.removeAttribute(attribute);
  }, [isShowingOnboarding]);

  // Reset the scroll position whenever the active section changes.
  useLayoutEffect(() => {
    settingsScrollRef.current?.scrollTo({ top: 0 });
  }, [currentSection]);

  useEffect(() => {
    checkOnboardingStatus();
  }, []);

  // Initialize RTL direction when language changes
  useEffect(() => {
    initializeRTL(i18n.language);
  }, [i18n.language]);

  // Initialize Enigo, shortcuts, and refresh audio devices when main app loads
  useEffect(() => {
    if (onboardingStep === "done" && !hasCompletedPostOnboardingInit.current) {
      hasCompletedPostOnboardingInit.current = true;
      Promise.all([
        commands.initializeEnigo(),
        commands.initializeShortcuts(),
      ]).catch((e) => {
        console.warn("Failed to initialize:", e);
      });
      refreshAudioDevices();
      refreshOutputDevices();
    }
  }, [onboardingStep, refreshAudioDevices, refreshOutputDevices]);

  // Handle keyboard shortcuts for debug mode toggle
  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      // Check for Ctrl+Shift+D (Windows/Linux) or Cmd+Shift+D (macOS)
      const isDebugShortcut =
        event.shiftKey &&
        event.key.toLowerCase() === "d" &&
        (event.ctrlKey || event.metaKey);

      if (isDebugShortcut) {
        event.preventDefault();
        const currentDebugMode = settings?.debug_mode ?? false;
        updateSetting("debug_mode", !currentDebugMode);
      }
    };

    // Add event listener when component mounts
    document.addEventListener("keydown", handleKeyDown);

    // Cleanup event listener when component unmounts
    return () => {
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [settings?.debug_mode, updateSetting]);

  // Listen for recording errors from the backend and show a toast
  useEffect(() => {
    const unlisten = listen<RecordingErrorEvent>("recording-error", (event) => {
      const { error_type, detail } = event.payload;

      if (error_type === "microphone_permission_denied") {
        const currentPlatform = platform();
        const platformKey = `errors.micPermissionDenied.${currentPlatform}`;
        const description = t(platformKey, {
          defaultValue: t("errors.micPermissionDenied.generic"),
        });
        toast.error(t("errors.micPermissionDeniedTitle"), { description });
      } else if (error_type === "no_input_device") {
        toast.error(t("errors.noInputDeviceTitle"), {
          description: t("errors.noInputDevice"),
        });
      } else {
        toast.error(
          t("errors.recordingFailed", { error: detail ?? "Unknown error" }),
        );
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [t]);

  // Listen for paste failures and show a toast.
  // The technical error detail is logged to transcreve-ai.log on the Rust side
  // (see actions.rs `error!("Failed to paste transcription: ...")`),
  // so we show a localized, user-friendly message here instead of the raw error.
  useEffect(() => {
    const unlisten = listen("paste-error", () => {
      toast.error(t("errors.pasteFailedTitle"), {
        description: t("errors.pasteFailed"),
      });
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [t]);

  // Listen for transcription failures and show a toast.
  // The payload is the backend error message (also logged to transcreve-ai.log).
  useEffect(() => {
    const unlisten = listen<string>("transcription-error", (event) => {
      toast.error(t("errors.transcriptionFailedTitle"), {
        description: event.payload,
      });
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [t]);

  // Listen for model loading failures and show a toast
  useEffect(() => {
    const unlisten = listen<ModelStateEvent>("model-state-changed", (event) => {
      if (event.payload.event_type === "loading_failed") {
        toast.error(
          t("errors.modelLoadFailed", {
            model:
              event.payload.model_name || t("errors.modelLoadFailedUnknown"),
          }),
          {
            description: event.payload.error,
          },
        );
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [t]);

  // Move to a section, optionally landing on a Settings tab. The tab goes
  // through `pendingSettingsTab`, which delivers it to a mounted hub or stashes
  // it for the hub this navigation is about to mount.
  const goTo = useCallback((navigation: Navigation) => {
    if (navigation.section) setCurrentSection(navigation.section);
    if (navigation.settingsTab) {
      handleNavigatePayload({ settingsTab: navigation.settingsTab });
    }
  }, []);

  // Toast/assistant actions deep-link a section via `hub://navigate`. Legacy
  // `models` redirects to Settings → Models. An explicit `settingsTab` in the
  // payload is already routed by `pendingSettingsTab`'s own listener, so only
  // a tab derived by the redirect is forwarded here.
  useEffect(() => {
    const unlisten = listen<{ section?: string; settingsTab?: string }>(
      "hub://navigate",
      (event) => {
        const navigation = resolveNavigation(event.payload);
        if (navigation.section) setCurrentSection(navigation.section);
        if (navigation.settingsTab && event.payload.settingsTab === undefined) {
          handleNavigatePayload({ settingsTab: navigation.settingsTab });
        }
      },
    );
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Ctrl/Cmd+1..5 switch sections, Ctrl/Cmd+, opens Settings.
  useEffect(() => {
    if (onboardingStep !== "done") return;
    const handleShortcut = (event: KeyboardEvent) => {
      const section = sectionForShortcut(event);
      if (!section) return;
      event.preventDefault();
      setCurrentSection(section);
    };
    document.addEventListener("keydown", handleShortcut);
    return () => document.removeEventListener("keydown", handleShortcut);
  }, [onboardingStep]);

  const revealMainWindowForPermissions = async () => {
    try {
      await commands.showMainWindowCommand();
    } catch (e) {
      console.warn("Failed to show main window for permission onboarding:", e);
    }
  };

  const checkOnboardingStatus = async () => {
    try {
      const settingsResult = await commands.getAppSettings();
      const hasCompletedOnboarding =
        settingsResult.status === "ok" &&
        settingsResult.data.onboarding_completed === true;
      const currentPlatform = platform();

      if (hasCompletedOnboarding) {
        // Returning user - check if they need to grant permissions first
        setIsReturningUser(true);

        if (currentPlatform === "macos") {
          try {
            const [hasAccessibility, hasMicrophone] = await Promise.all([
              checkAccessibilityPermission(),
              checkMicrophonePermission(),
            ]);
            if (!hasAccessibility || !hasMicrophone) {
              await revealMainWindowForPermissions();
              setOnboardingStep("accessibility");
              return;
            }
          } catch (e) {
            console.warn("Failed to check macOS permissions:", e);
            // If we can't check, proceed to main app and let them fix it there
          }
        }

        if (currentPlatform === "windows") {
          try {
            const microphoneStatus =
              await commands.getWindowsMicrophonePermissionStatus();
            if (
              microphoneStatus.status === "ok" &&
              microphoneStatus.data.supported &&
              microphoneStatus.data.overall_access === "denied"
            ) {
              await revealMainWindowForPermissions();
              setOnboardingStep("accessibility");
              return;
            }
          } catch (e) {
            console.warn("Failed to check Windows microphone permissions:", e);
            // If we can't check, proceed to main app and let them fix it there
          }
        }

        setOnboardingStep("done");
      } else {
        // New user - start full onboarding
        setIsReturningUser(false);
        setOnboardingStep("accessibility");
      }
    } catch (error) {
      console.error("Failed to check onboarding status:", error);
      setOnboardingStep("accessibility");
    }
  };

  const handleAccessibilityComplete = () => {
    // Returning users already have models, skip to main app
    // New users need to select a model
    setOnboardingStep(isReturningUser ? "done" : "model");
  };

  const handleModelSelected = () => {
    // Transition to main app - user has started a download
    setOnboardingStep("done");
  };

  // Rendered once around every step below (including onboarding) so
  // toast.error() calls surface to the user. sonner renders via a portal, so
  // its position in the tree doesn't affect layout. Without this, errors during
  // onboarding (e.g. a model download failing because blob.handy.computer is
  // unreachable) are silently swallowed and the wizard just appears to "blink".
  const toaster = (
    <Toaster
      theme="system"
      toastOptions={{
        unstyled: true,
        classNames: {
          toast:
            "bg-background border border-mid-gray/20 rounded-lg shadow-lg px-4 py-3 flex items-center gap-3 text-sm",
          title: "font-medium",
          description: "text-mid-gray",
          actionButton:
            "px-2 py-1 text-xs font-medium rounded-lg border bg-mid-gray/10 border-mid-gray/20 hover:bg-background-ui/30 hover:border-logo-primary cursor-pointer whitespace-nowrap",
        },
      }}
    />
  );

  // Still checking onboarding status
  if (onboardingStep === null) {
    return null;
  }

  // Select the content for the current step. The Toaster is rendered once, in a
  // stable wrapper around this node, so crossing between onboarding steps and
  // the main app never remounts it (which would drop any in-flight toast).
  let content: ReactNode;
  if (onboardingPreview) {
    // Render previews in the same top-level slot as real onboarding. Keeping
    // the settings layout unmounted ensures viewport overflow behaves exactly
    // as it does during first-run onboarding.
    content = (
      <>
        {onboardingPreview === "accessibility" ? (
          <AccessibilityOnboarding onComplete={NOOP} preview />
        ) : (
          <Onboarding onModelSelected={NOOP} preview />
        )}
        <button
          type="button"
          onClick={() => setOnboardingPreview(null)}
          className="fixed top-4 end-4 z-50 rounded-lg border border-mid-gray/20 bg-background px-4 py-2 text-sm font-medium text-text shadow-lg hover:bg-background-ui/30 cursor-pointer"
        >
          {t("settings.debug.onboardingPreview.exitButton")}
        </button>
      </>
    );
  } else if (onboardingStep === "accessibility") {
    content = (
      <AccessibilityOnboarding onComplete={handleAccessibilityComplete} />
    );
  } else if (onboardingStep === "model") {
    content = <Onboarding onModelSelected={handleModelSelected} />;
  } else {
    content = (
      <HubNavigationContext.Provider value={goTo}>
        <div dir={direction} className="hub-shell select-none cursor-default">
          <ErrorBoundary context="What's New">
            <WhatsNewGate />
          </ErrorBoundary>
          <ErrorBoundary context="Meeting Consent">
            <MeetingConsentGate />
          </ErrorBoundary>
          {/* Canvas (rail) + inset panel (content) */}
          <div className="hub-body">
            <Sidebar
              activeSection={currentSection}
              onSectionChange={setCurrentSection}
              onNavigate={goTo}
              collapsed={railCollapsed}
              onToggleCollapsed={toggleRailCollapsed}
            />
            <div className="hub-panel">
              <div
                ref={settingsScrollRef}
                className={
                  isFullBleed
                    ? "hub-panel-scroll overflow-hidden"
                    : "hub-panel-scroll overflow-y-auto"
                }
              >
                <div
                  className={
                    isFullBleed
                      ? "h-full"
                      : "flex flex-col items-center p-4 gap-4"
                  }
                >
                  {!isFullBleed ? <AccessibilityPermissions /> : null}
                  {!isFullBleed ? <SecureInputWarning /> : null}
                  {renderSettingsContent(currentSection)}
                </div>
              </div>
            </div>
          </div>
          {/* Fixed footer at bottom */}
          <Footer />
        </div>
      </HubNavigationContext.Provider>
    );
  }

  return (
    <>
      {toaster}
      {content}
    </>
  );
}

export default App;
