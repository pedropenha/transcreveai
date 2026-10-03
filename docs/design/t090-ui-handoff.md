# T-090 UI handoff

- Owner: t090_ui; state: Review; checkout: `C:\multimidia\ecc`, `integration/v1`.
- Scope: assistant provider availability, CLI configuration fields, independent configuration chooser, narrowly added localized strings and test/coverage wiring. No commits or branch changes.

## Behavior

- Assistant CLI selection requires detected and enabled status, including experimental adapters. Unknown detection remains disabled until the status arrives.
- Experimental enable/disable is available. Summary options remain disabled for experimental agents.
- The Assistant settings page offers a separate configuration selector. Missing CLI agents remain unavailable for invocation but can receive a binary override without changing the assistant or summary provider.
- Successful saves refresh settings and CLI detection. Save failures show a localized message and retain the draft. Changing the configuration provider resets input drafts and errors through a keyed fields component.

## Evidence

- RED: focused tests failed for the missing availability helper and the experimental toggle being disabled before implementation.
- GREEN: 8 focused tests pass. `bunx tsc --noEmit` and targeted ESLint pass.
- Chromium E2E passes with mocked Tauri IPC: missing experimental is disabled; binary override refreshes detection; detected-but-disabled stays disabled; keyboard enable makes explicit selection available; configuration changes never select the summary/assistant provider; arguments, timeout and model save; structured and thrown rejected saves keep draft and hide backend details; switching configuration resets draft; explicit assistant provider selection persists.
- Browser V8 coverage, mapped to current source and checked by `scripts/check-cli-settings-coverage.ts`: AssistantProvider 100% lines/functions, 83.33% branches; CliAgentConfiguration 100% lines/functions, 93.75% branches; CliAgentFields 100% lines/functions, 86.36% branches. All measured metrics meet 80%.
- Pure assistant availability helper: 100% lines/functions in focused Bun coverage.
- Final unit wrapper repair: Bun describe/test suites are imported at module top level, outside standalone-test callbacks. `bun scripts/check-frontend-coverage.ts` passes the existing four-module ratchet; `bun run test:coverage` passes 21 tests with 82.37% functions and 90.87% lines overall for the instrumented modules. Component callback coverage comes from the browser report above, not the SSR-only unit report.
- `git diff --check` passes (only pre-existing CRLF notices in unrelated Rust files).
- Independent security reviewer t090_security inspected the final selection/configuration boundary and reported no high/critical UI security findings.

## Limits

Browser tests use mocked IPC. They verify UI transitions and persistence command payloads, not native CLI execution or real subscription authentication. Both structured backend-error results and thrown transport errors are exercised; messages are localized and drafts retained. Native acceptance belongs to the integrator/backend card.

## Files

Production: AssistantProvider.tsx, assistantProviderOptions.ts, CliAgentConfiguration.tsx, CliAgentFields.tsx; new `configure` locale keys in en/pt-BR. Tests: assistantProviderOptions.test.ts, cliAgentFields.test.tsx, cli-agent-settings.spec.ts, cli-settings-coverage.ts, check-cli-settings-coverage.ts. Added focused tests to package.json and scripts/unit-coverage.test.ts.
