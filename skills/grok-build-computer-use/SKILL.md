---
name: grok-build-computer-use
description: Operate and verify desktop applications through Grok Build computer-use tools. Use for browser navigation, visual UI testing, rendered-interface debugging, and multi-step desktop workflows where screenshots, accessibility targets, keyboard input, pixels, or plan_click must be chosen deliberately.
---

# Grok Build Computer Use

Use computer use as a closed loop: observe, decide, act through the highest-level reliable control, then verify the rendered outcome. A successful action receipt means input was dispatched; it does not prove the intended UI effect occurred.

## Choose the control surface

Use the highest-level control that can complete the goal:

1. Use an accessibility element and `AXPress` for stable, named native controls.
2. Use keyboard navigation, browser shortcuts, direct URLs, or a site query when available.
3. Use a planned pixel click only when the page canvas has no reliable semantic target.

For web work, treat browser chrome as the reliable surface. Prefer omnibox focus, URL/query entry, and Enter over hunting links or product cards in page pixels. Do not wait indefinitely for web content to appear in the accessibility tree.

Do not use computer use to open or operate System Settings.

## Snapshot and planning protocol

Use `get_app_state` before each independent action sequence. Coordinates use a top-left PNG origin: x increases right and y increases down.

For an uncertain target, use this handoff:

```text
get_app_state        -> snapshot A: identify intent
plan_click(A,target) -> snapshot B: fresh capture with black cursor preview
click(B,resolved target) -> dispatch
```

`plan_click` captures fresh state itself. Its input snapshot is an intent hint, not authorization for the final action. Read its output and use the returned `snapshot_id` and resolved target; never click snapshot A after planning. For pixel targets, use B-space coordinates, not the original A-space point.

The black pseudo-cursor's tip is the resolved top-left PNG point. It answers “where was the target resolved in the fresh UI?” It does not prove that a page will navigate. For an AX target, dispatch is semantic `AXPress`; the cursor is explanatory only, not a physical-click promise.

If the source snapshot is unavailable, consumed, or refers to the wrong window, obtain fresh app state. If planning cannot uniquely resolve an element in the fresh capture, do not guess; observe again and choose a current target.

Do not invoke `plan_click` mechanically. Use it for ambiguous, small, dense, or visually unstable targets. For a known AX browser-chrome control or a keyboard shortcut, use the higher-level control directly.

## Verify effects, not inputs

After every consequential action, acquire fresh state and verify the user-visible success condition.

- Navigation: verify URL, title, heading, or a page-specific landmark.
- Product detail: verify product identity plus evidence such as price or rating.
- Form/action: verify the changed value, toast, dialog, or resulting state.
- Debugging: verify the visual regression or expected rendering in the isolated target environment.

If the UI scrolls, expands an AI panel, loads content, changes zoom, or otherwise reflows, discard prior pixel coordinates. Re-observe and re-plan.

## Browser workflows

Encode navigation as a URL or search query when possible:

1. Focus the omnibox or browser search control semantically or with a shortcut.
2. Select all, type the full URL or query, and submit with Enter.
3. Observe the result.
4. Verify page type and identity before proceeding.

For comparison tasks, separate selection from navigation:

1. Capture a baseline list/page state.
2. Gather a bounded candidate table: identity, primary price, rating, and constraints.
3. Apply the rule explicitly.
4. Navigate to the chosen candidate and verify the final page matches it.

Define ambiguous terms before selecting—for example, primary shelf price versus struck-through price, and whether adjacent categories count.

## Visual testing and debugging

Run each test scenario in its intended sandbox and keep its acceptance criteria explicit. Capture evidence before and after the action: the rendered component, relevant state, and any console/UI error exposed by the task. For parallel runs, assign disjoint scenarios or isolated environments; do not infer a global regression from one sandbox's transient state.

Use a minimal diagnostic loop:

1. Reproduce the visible condition.
2. Record the expected versus actual rendered state.
3. Make or trigger one bounded change.
4. Re-observe and verify the exact condition changed.

Do not let an agent “fix” a visual problem by navigating away, changing the query, or switching environments unless that is the requested test outcome.

## Recovery rules

| Signal | Response |
| --- | --- |
| `invalid_snapshot` or expired snapshot | Acquire fresh app state; restart the A → B planning handoff. |
| Cursor misses after a layout change | Re-observe and re-plan; never reuse the old pixel target. |
| Input applied but page unchanged | Escalate control level: semantic/keyboard URL or site search; then verify. |
| AX exposes only browser chrome | Use browser chrome or keyboard; treat page canvas as pixel-only and verify carefully. |
| Window-bind/state-unavailable error | Re-acquire app state; do not replay the prior action. |

Avoid repeated blind clicks, double-clicking links as a fallback, and using a marker as proof of navigation. Every retry must be based on newly observed state.

Before declaring success, challenge the result: did the target page or component actually change, is the evidence from the intended sandbox, and would the same verification distinguish a no-op click from a real success? If not, collect a stronger observable.
