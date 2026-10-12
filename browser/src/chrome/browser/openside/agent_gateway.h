// Copyright 2026 The openside authors
// SPDX-License-Identifier: MIT OR Apache-2.0

#ifndef CHROME_BROWSER_OPENSIDE_AGENT_GATEWAY_H_
#define CHROME_BROWSER_OPENSIDE_AGENT_GATEWAY_H_

class BrowserWindowInterface;
class GURL;

namespace content {
class DevToolsAgentHost;
class WebContents;
}  // namespace content

namespace openside {

// The gateway exists only in a daemon-launched browser that passes this switch.
inline constexpr char kGatewayInstanceSwitch[] = "openside-gateway-instance-id";
inline constexpr char kAgentSessionSwitch[] = "openside-agent-session-id";
inline constexpr char kAgentSubagentSwitch[] = "openside-agent-subagent-id";
// Absolute path of a real directory (not a symlink), owned by the user with
// mode exactly 0700, that holds openside/browser/<instance-id>/. Without the
// switch the gateway falls back to XDG_RUNTIME_DIR.
inline constexpr char kGatewayRuntimeDirSwitch[] =
    "openside-gateway-runtime-dir";

// Linux only: the definitions are compiled only when is_linux (see BUILD.gn),
// and every call site is guarded by BUILDFLAG(IS_LINUX).
// UI thread only. Both are no-ops without kGatewayInstanceSwitch.
void StartAgentGateway();
void ShutdownAgentGateway();

// Empty unless the explicit UI switch names a credential-free loopback origin.
GURL DaemonUiUrl();

// Called once per tab when Chrome attaches its tab helpers.
void OnTabCreated(content::WebContents* contents);

// Records that `opener` created `opened`. Contents opened directly or
// transitively by an agent tab inherit its focus restrictions, never its
// ownership or authority.
void NoteOpenedContents(content::WebContents* opener,
                        content::WebContents* opened);

// Called after TabOwnership changed owner, so state that only agent tabs
// carry (the 1440x900 viewport emulation) is dropped at once.
void OnTabOwnerChanged(content::WebContents* contents);

// Called for every DevTools session that leaves `host`, after its handlers
// were disabled. A leaving session's EmulationHandler clears the page-wide
// device metrics, scale factor and mobile mode included, even when the view
// size stays the same, so an agent tab's 1440x900 override is re-applied.
void OnDevToolsClientDetached(content::DevToolsAgentHost* host);

// Two actors ask for activation. In an agent browser the DevTools endpoint
// belongs to the daemon, so a DevTools protocol activation
// (content::IsProtocolActivation()) is an agent request whatever tab it
// names. Pages ask through window.focus(), popups and fullscreen; agent-owned
// tabs and their popup descendants never get those. False for either; a
// denied attempt is reported over the gateway as `focus.decided`.
bool MayActivateTab(content::WebContents* contents);

// False in an agent browser: DevTools clients there are the agent, which
// never resizes a window or changes its state (that would raise or restore
// the user's window, or move the fixed agent viewport).
bool MayChangeWindowsOverProtocol();

// False when every tab of a non-empty window is agent-owned or
// agent-originated: such a window is shown inactive, and no programmatic
// request (CDP, extension APIs, popups, tab switches) raises, activates,
// resizes, changes the state of, or flashes it. Extensions act for the user
// on windows that hold a user tab. The user still activates any window
// through the window manager. A denied attempt is reported over the gateway
// as `focus.decided`.
bool MayActivateBrowserWindow(BrowserWindowInterface* window);

// False for downloads started by agent-originated contents: their details
// bubble would take keyboard focus from the page the user is on.
bool MayShowDownloadUi(content::WebContents* source);

}  // namespace openside

#endif  // CHROME_BROWSER_OPENSIDE_AGENT_GATEWAY_H_
