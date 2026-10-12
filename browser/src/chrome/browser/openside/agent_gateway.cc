// Copyright 2026 The openside authors
// SPDX-License-Identifier: MIT OR Apache-2.0

#include "chrome/browser/openside/agent_gateway.h"

#include <errno.h>
#include <fcntl.h>
#include <stdlib.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/un.h>
#include <unistd.h>

#include <algorithm>
#include <array>
#include <cstdint>
#include <deque>
#include <map>
#include <memory>
#include <limits>
#include <optional>
#include <set>
#include <span>
#include <string>
#include <string_view>
#include <tuple>
#include <utility>
#include <vector>

#include "base/base64.h"
#include "base/command_line.h"
#include "base/feature_list.h"
#include "base/files/file_descriptor_watcher_posix.h"
#include "base/files/scoped_file.h"
#include "base/functional/bind.h"
#include "base/functional/callback_helpers.h"
#include "base/json/json_reader.h"
#include "base/json/json_writer.h"
#include "base/logging.h"
#include "base/memory/raw_ptr.h"
#include "base/memory/weak_ptr.h"
#include "base/no_destructor.h"
#include "base/posix/eintr_wrapper.h"
#include "base/numerics/byte_conversions.h"
#include "base/rand_util.h"
#include "base/strings/string_number_conversions.h"
#include "base/strings/utf_string_conversions.h"
#include "base/task/bind_post_task.h"
#include "base/task/sequenced_task_runner.h"
#include "base/task/thread_pool.h"
#include "base/time/time.h"
#include "base/timer/timer.h"
#include "base/values.h"
#include "chrome/browser/lifetime/browser_shutdown.h"
#include "chrome/browser/openside/focus_arbiter.h"
#include "chrome/browser/openside/tab_ownership.h"
#include "chrome/browser/profiles/profile.h"
#include "chrome/browser/search_engines/template_url_service_factory.h"
#include "chrome/browser/ui/side_panel/side_panel_entry.h"
#include "chrome/browser/ui/side_panel/side_panel_registry.h"
#include "chrome/browser/ui/tabs/features.h"
#include "chrome/common/pref_names.h"
#include "chrome/browser/ui/browser_window/public/browser_window_interface.h"
#include "chrome/browser/ui/browser_window/public/browser_window_interface_iterator.h"
#include "chrome/browser/ui/tabs/tab_group_model.h"
#include "chrome/browser/ui/tabs/tab_strip_model.h"
#include "components/autofill/core/common/form_data.h"
#include "components/autofill/core/common/form_field_data.h"
#include "components/password_manager/content/browser/content_password_manager_driver.h"
#include "components/download/content/public/all_download_item_notifier.h"
#include "components/download/public/common/download_item.h"
#include "components/infobars/content/content_infobar_manager.h"
#include "components/infobars/core/infobar.h"
#include "components/prefs/pref_service.h"
#include "components/javascript_dialogs/openside_dialog_hook.h"
#include "components/sessions/core/session_id.h"
#include "components/search_engines/template_url.h"
#include "components/search_engines/template_url_data.h"
#include "components/search_engines/template_url_service.h"
#include "components/tab_groups/tab_group_color.h"
#include "components/tab_groups/tab_group_id.h"
#include "components/tab_groups/tab_group_visual_data.h"
#include "components/tabs/public/tab_group.h"
#include "components/tabs/public/tab_interface.h"
#include "components/viz/common/frame_sinks/copy_output_result.h"
#include "content/public/browser/browser_context.h"
#include "content/public/browser/devtools_agent_host.h"
#include "content/public/browser/devtools_agent_host_client.h"
#include "content/public/browser/download_item_utils.h"
#include "content/public/browser/download_manager.h"
#include "content/public/browser/render_frame_host.h"
#include "content/public/browser/openside_protocol_activation.h"
#include "content/public/browser/weak_document_ptr.h"
#include "content/public/browser/render_widget_host_view.h"
#include "content/public/browser/visibility.h"
#include "content/public/browser/web_contents.h"
#include "content/public/browser/web_contents_observer.h"
#include "content/public/browser/web_contents_user_data.h"
#include "third_party/skia/include/core/SkBitmap.h"
#include "ui/base/base_window.h"
#include "ui/gfx/codec/png_codec.h"
#include "ui/gfx/geometry/rect.h"
#include "ui/gfx/geometry/size.h"
#include "url/gurl.h"
#include "url/origin.h"

namespace openside {

namespace {

constexpr char kProtocol[] = "openside.browser";
constexpr char16_t kAgentTabsTitle[] = u"Agent Tabs";
constexpr size_t kMaxFrameBytes = 16 * 1024 * 1024;
// Requests are small; capping inbound frames far below the outbound limit
// bounds both retained input and synchronous JSON parsing on the UI thread.
constexpr size_t kMaxRequestFrameBytes = 1024 * 1024;
constexpr size_t kMaxHelloFrameBytes = 4096;
constexpr size_t kMaxInputBytes = 4 + kMaxRequestFrameBytes;
// Per UI task: socket bytes read and frames handled before yielding.
constexpr size_t kReadBudgetBytes = 256 * 1024;
constexpr int kFramesPerTask = 32;
constexpr size_t kMaxCapturesInFlight = 2;
// Reuse detection covers this many most recent request ids per connection.
constexpr size_t kRequestIdWindow = 1024;
constexpr size_t kMaxTerminalDownloads = 1024;
// Largest integer a JSON peer can represent exactly (2^53 - 1).
constexpr uint64_t kMaxSequence = (uint64_t{1} << 53) - 1;
// A window manager that refuses the agent size must not cause a resize loop,
// and a client that keeps overriding an agent page is not fought forever.
constexpr int kMaxViewportCorrections = 8;
constexpr size_t kMaxQueuedFrames = 64;
// Requests are not handled while this many frames wait for the client.
constexpr size_t kPauseQueuedFrames = 16;
constexpr base::TimeDelta kStalledReaderTimeout = base::Seconds(5);
constexpr size_t kMaxQueuedBytes = 32 * 1024 * 1024;
constexpr size_t kMaxPngBytes = 8 * 1024 * 1024;
constexpr size_t kMaxRequestIdBytes = 128;
constexpr int kAgentViewportWidth = 1440;
constexpr int kAgentViewportHeight = 900;
constexpr gfx::Size kAgentViewport(kAgentViewportWidth, kAgentViewportHeight);
constexpr base::TimeDelta kHandshakeTimeout = base::Seconds(5);
// End to end: viewport preparation, the surface copy and encoding. Enforced
// by a posted task, so a stage that completes before that task runs on a
// delayed UI thread may still succeed slightly late.
constexpr base::TimeDelta kCaptureTimeout = base::Seconds(8);
// A renderer whose main thread is busy answers no protocol command.
constexpr base::TimeDelta kEmulationAckTimeout = base::Seconds(3);
constexpr base::TimeDelta kBoundsSettleDelay = base::Milliseconds(500);

const char* OwnerName(TabOwner owner) {
  switch (owner) {
    case TabOwner::kAgent:
      return "agent";
    case TabOwner::kUser:
      return "user";
    case TabOwner::kExternal:
      return "external";
  }
  return "external";
}

const char* OpenedByName(OpenedBy opened_by) {
  switch (opened_by) {
    case OpenedBy::kAgent:
      return "agent";
    case OpenedBy::kUser:
      return "user";
    case OpenedBy::kExternal:
      return "external";
  }
  return "external";
}

const char* FocusReason(FocusDecision decision) {
  switch (decision) {
    case FocusDecision::kAllow:
      return "allowed";
    case FocusDecision::kDenyInvalidIdentity:
      return "invalid_identity";
    case FocusDecision::kDenyNoUserGrant:
      return "no_user_grant";
    case FocusDecision::kDenyDifferentOwner:
      return "different_owner";
  }
  return "no_user_grant";
}

base::Value Nullable(const std::string& value) {
  return value.empty() ? base::Value() : base::Value(value);
}

// base::Value has no 64-bit integer. Larger counters travel as doubles, which
// are exact up to 2^53 and are written without a fraction (see ToJson).
base::Value Count(uint64_t value) {
  return value <= static_cast<uint64_t>(std::numeric_limits<int>::max())
             ? base::Value(static_cast<int>(value))
             : base::Value(static_cast<double>(value));
}

std::optional<std::string> ToJson(base::ValueView value) {
  std::string json;
  if (!base::JSONWriter::WriteWithOptions(
          value, base::JSONWriter::OPTIONS_OMIT_DOUBLE_TYPE_PRESERVATION,
          &json)) {
    return std::nullopt;
  }
  return json;
}

std::string LaunchSwitch(const char* name) {
  return base::CommandLine::ForCurrentProcess()->GetSwitchValueASCII(name);
}

void RegisterUiKeywords(TemplateURLService* service, const GURL& ui) {
  for (const auto& [keyword, name, suffix] :
       {std::tuple{u"ask", u"Ask openside", "?q={searchTerms}"},
        std::tuple{u"routines", u"openside routines",
                   "routines?q={searchTerms}"}}) {
    // An explicit user keyword always wins over the opt-in integration.
    if (service->GetTemplateURLForKeyword(keyword)) {
      continue;
    }
    TemplateURLData data;
    data.SetShortName(name);
    data.SetKeyword(keyword);
    data.SetURL(ui.spec() + suffix);
    data.safe_for_autoreplace = false;
    auto* added = service->Add(std::make_unique<TemplateURL>(data));
    if (added) {
      service->SetIsActiveTemplateURL(added, true);
    }
  }
}

bool IsAgentTab(content::WebContents* contents) {
  const TabOwnership* ownership = TabOwnership::FromWebContents(contents);
  return ownership && ownership->owner() == TabOwner::kAgent;
}

// Focus provenance for contents opened, directly or transitively, by an agent
// tab. It grants no agent authority: such popups stay owner:external.
class AgentOrigin : public content::WebContentsUserData<AgentOrigin> {
 private:
  friend class content::WebContentsUserData<AgentOrigin>;
  explicit AgentOrigin(content::WebContents* contents)
      : content::WebContentsUserData<AgentOrigin>(*contents) {}
  WEB_CONTENTS_USER_DATA_KEY_DECL();
};

WEB_CONTENTS_USER_DATA_KEY_IMPL(AgentOrigin);

bool IsAgentOriginated(content::WebContents* contents) {
  return contents &&
         (IsAgentTab(contents) || AgentOrigin::FromWebContents(contents));
}

bool ValidInstanceId(std::string_view id) {
  if (id.size() < 8 || id.size() > 64) {
    return false;
  }
  for (char c : id) {
    if (!((c >= '0' && c <= '9') || (c >= 'a' && c <= 'z') || c == '-')) {
      return false;
    }
  }
  return true;
}

// A real directory (never a symlink) owned by the running UID with mode
// exactly 0700, the same rule the controller applies before passing the
// switch. Opened once and checked through the descriptor, so everything
// below it is created relative to what was checked rather than re-resolved
// by path; a renamed or swapped ancestor can only redirect the open to
// another directory that passes the same check.
base::ScopedFD OpenPrivateDirectory(int parent, const char* name) {
  base::ScopedFD fd(HANDLE_EINTR(openat(
      parent, name, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)));
  struct stat info;
  if (!fd.is_valid() || fstat(fd.get(), &info) != 0 ||
      !S_ISDIR(info.st_mode) || info.st_uid != geteuid() ||
      (info.st_mode & 07777) != 0700) {
    return base::ScopedFD();
  }
  return fd;
}

base::ScopedFD EnsurePrivateDirectory(int parent, const char* name) {
  if (mkdirat(parent, name, 0700) != 0 && errno != EEXIST) {
    return base::ScopedFD();
  }
  return OpenPrivateDirectory(parent, name);
}

bool WriteExclusive(int directory, const char* name, std::string_view contents) {
  int fd = HANDLE_EINTR(openat(directory, name,
                               O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW |
                                   O_CLOEXEC,
                               0600));
  if (fd < 0) {
    return false;
  }
  bool ok = fchmod(fd, 0600) == 0;
  size_t written = 0;
  while (ok && written < contents.size()) {
    ssize_t n = write(fd, contents.data() + written, contents.size() - written);
    if (n < 0 && errno == EINTR) {
      continue;
    }
    ok = n > 0;
    if (ok) {
      written += static_cast<size_t>(n);
    }
  }
  ok = close(fd) == 0 && ok;
  if (!ok) {
    unlinkat(directory, name, 0);
  }
  return ok;
}

bool ConstantTimeEquals(std::string_view a, std::string_view b) {
  if (a.size() != b.size()) {
    return false;
  }
  unsigned char diff = 0;
  for (size_t i = 0; i < a.size(); ++i) {
    diff |= static_cast<unsigned char>(a[i] ^ b[i]);
  }
  return diff == 0;
}

// The single owner of an agent page's 1440x900 layout. In the patched
// browser the controller does not override agent tabs' device metrics (see
// Browser::attach), so this session is the only one that should; any other
// DevTools client that sets, changes or clears an override is undone the
// next time the tab is checked. On Wayland and under tiling window managers
// the compositor owns the toplevel size, so a window alone cannot be relied
// on to reach 1440x900.
class ViewportEmulation : public content::DevToolsAgentHostClient {
 public:
  // `on_applied` runs in a later task with whether the renderer accepted the
  // override within kEmulationAckTimeout.
  ViewportEmulation(content::WebContents* contents,
                    base::RepeatingCallback<void(bool)> on_applied)
      : host_(content::DevToolsAgentHost::GetOrCreateFor(contents)),
        on_applied_(std::move(on_applied)) {
    if (!host_->AttachClient(this)) {
      host_ = nullptr;
    }
  }
  ViewportEmulation(const ViewportEmulation&) = delete;
  ViewportEmulation& operator=(const ViewportEmulation&) = delete;
  ~ViewportEmulation() override {
    if (host_) {
      host_->DetachClient(this);
    }
  }

  bool attached() const { return !!host_; }
  bool applying() const { return pending_set_ != 0; }

  // Clears this session's override before setting it again. Another
  // session's clear or detach resets the WebContents-wide emulation, while
  // this session's EmulationHandler still caches its last parameters and
  // answers an identical set without re-applying them; the clear drops that
  // cache so the set reaches the renderer again, scale factor included.
  void Restore() {
    if (!host_ || pending_set_) {
      return;
    }
    Send("Emulation.clearDeviceMetricsOverride", base::DictValue());
    base::DictValue params;
    params.Set("width", kAgentViewportWidth);
    params.Set("height", kAgentViewportHeight);
    params.Set("deviceScaleFactor", 1);
    params.Set("mobile", false);
    // Set before dispatching: a reply may arrive synchronously. Ids only
    // grow, so a reply that arrives after its timeout never matches a newer
    // request.
    pending_set_ = next_id_ + 1;
    ack_timeout_.Start(FROM_HERE, kEmulationAckTimeout,
                       base::BindOnce(&ViewportEmulation::Finish,
                                      base::Unretained(this), false));
    if (!Send("Emulation.setDeviceMetricsOverride", std::move(params))) {
      Finish(false);
    }
  }

  void DispatchProtocolMessage(content::DevToolsAgentHost* host,
                               base::span<const uint8_t> message) override {
    if (!pending_set_) {
      return;
    }
    std::optional<base::DictValue> reply = base::JSONReader::ReadDict(
        std::string_view(reinterpret_cast<const char*>(message.data()),
                         message.size()),
        base::JSON_PARSE_RFC);
    if (!reply || reply->FindInt("id") != pending_set_) {
      return;
    }
    Finish(!reply->Find("error"));
  }
  void AgentHostClosed(content::DevToolsAgentHost* host) override {
    host_ = nullptr;
    if (pending_set_) {
      Finish(false);
    }
  }

 private:
  bool Send(std::string_view method, base::DictValue params) {
    base::DictValue message;
    message.Set("id", ++next_id_);
    message.Set("method", method);
    message.Set("params", std::move(params));
    std::optional<std::string> json = ToJson(message);
    if (!json || !host_) {
      return false;
    }
    host_->DispatchProtocolMessage(this, base::as_byte_span(*json));
    return true;
  }

  void Finish(bool applied) {
    ack_timeout_.Stop();
    pending_set_ = 0;
    base::SequencedTaskRunner::GetCurrentDefault()->PostTask(
        FROM_HERE, base::BindOnce(on_applied_, applied));
  }

  scoped_refptr<content::DevToolsAgentHost> host_;
  base::RepeatingCallback<void(bool)> on_applied_;
  int next_id_ = 0;
  int pending_set_ = 0;
  base::OneShotTimer ack_timeout_;
};

class GatewayTab;

std::map<uint64_t, raw_ptr<GatewayTab>>& Registry() {
  static base::NoDestructor<std::map<uint64_t, raw_ptr<GatewayTab>>> registry;
  return *registry;
}

uint64_t g_next_tab = 1;

class Gateway;
Gateway* g_gateway = nullptr;

void Emit(std::string_view type,
          GatewayTab* tab,
          base::DictValue payload);

// Per-WebContents gateway identity and lifecycle reporting. Lives exactly as
// long as the WebContents, so tab_id is stable across navigation and
// tab-strip moves, and is never reused within the instance.
class GatewayTab : public content::WebContentsObserver,
                   public content::WebContentsUserData<GatewayTab> {
 public:
  ~GatewayTab() override { Registry().erase(sequence_); }

  const std::string& tab_id() const { return tab_id_; }
  uint64_t primary_generation() const { return primary_generation_; }

  base::DictValue Record() {
    content::WebContents* contents = web_contents();
    const TabOwnership* ownership = TabOwnership::FromWebContents(contents);
    base::DictValue record;
    record.Set("tab_id", tab_id_);
    record.Set("cdp_target_id",
               content::DevToolsAgentHost::GetOrCreateFor(contents)->GetId());
    record.Set("owner",
               OwnerName(ownership ? ownership->owner() : TabOwner::kExternal));
    record.Set("session_id",
               Nullable(ownership ? ownership->session_id() : std::string()));
    record.Set("subagent_id",
               Nullable(ownership ? ownership->subagent_id() : std::string()));
    record.Set("label", ownership ? ownership->label() : std::string());
    record.Set("background", ownership ? ownership->background() : false);
    record.Set("opened_by", OpenedByName(ownership ? ownership->opened_by()
                                                   : OpenedBy::kExternal));
    record.Set("url", contents->GetLastCommittedURL().spec());
    record.Set("title", base::UTF16ToUTF8(contents->GetTitle()));
    record.Set("state", contents->WasDiscarded() ? "asleep" : "live");
    std::optional<std::u16string> group = GroupTitle();
    record.Set("group_title", group ? base::Value(base::UTF16ToUTF8(*group))
                                    : base::Value());
    return record;
  }

  // Runs after the creating task returns, so the CDP creation hook has
  // already attached TabOwnership when the tab is classified.
  void Announce() {
    content::WebContents* contents = web_contents();
    if (IsAgentTab(contents)) {
      // A hidden capturer keeps a background agent tab rendering without
      // making it visible or active; it never holds a wake lock.
      keep_rendering_ = contents->IncrementCapturerCount(
          gfx::Size(), /*stay_hidden=*/true, /*stay_awake=*/false,
          /*is_activity=*/false);
      JoinAgentTabsGroup();
      WatchTabStrip();
      ScheduleViewportFix();
    }
    announced_ = true;
    base::DictValue opened;
    opened.Set("tab", Record());
    Emit("tab.opened", this, std::move(opened));

    content::WebContents* opener =
        contents->HasLiveOriginalOpenerChain()
            ? contents->GetFirstWebContentsInLiveOriginalOpenerChain()
            : nullptr;
    GatewayTab* opener_tab = opener ? FromWebContents(opener) : nullptr;
    if (opener_tab) {
      base::DictValue popup;
      popup.Set("opener_tab_id", opener_tab->tab_id());
      popup.Set("child_tab", Record());
      popup.Set("blocked", false);
      Emit("popup.opened", nullptr, std::move(popup));
    }
  }

  std::string OpenDialog() {
    open_dialog_ = tab_id_ + "-dialog-" + base::NumberToString(next_dialog_++);
    return open_dialog_;
  }

  std::string TakeDialog() { return std::exchange(open_dialog_, std::string()); }

  // Agent pages lay out at 1440x900 CSS pixels at device scale factor 1 for
  // the whole tab lifetime. Every resize, visibility or tab-strip change
  // re-checks the layout after the triggering task.
  void ScheduleViewportFix() {
    // A tab that stopped being an agent tab still runs one fix to drop its
    // emulation.
    if (viewport_fix_pending_ ||
        (!IsAgentTab(web_contents()) && !emulation_)) {
      return;
    }
    viewport_fix_pending_ = true;
    base::SequencedTaskRunner::GetCurrentDefault()->PostTask(
        FROM_HERE, base::BindOnce(&GatewayTab::FixAgentViewport,
                                  weak_factory_.GetWeakPtr()));
  }

  // Another DevTools session left the tab. Its EmulationHandler cleared the
  // page-wide override, which may change only the scale factor or mobile
  // mode, so the view size alone cannot tell.
  void ReassertViewport() {
    if (!emulation_) {
      return;
    }
    reassert_ = true;
    ScheduleViewportFix();
  }

  // Runs `ready(true)` once this browser's override has been re-applied and
  // acknowledged by the renderer, so a capture never reflects metrics another
  // client left behind, or `ready(false)` when that did not happen. Each
  // capture starts a new correction budget.
  void PrepareCapture(base::OnceCallback<void(bool)> ready) {
    if (!IsAgentTab(web_contents())) {
      std::move(ready).Run(false);
      return;
    }
    capture_waiters_.push_back(std::move(ready));
    restores_ = 0;
    reassert_ = true;
    ScheduleViewportFix();
  }

  // Ownership left the agent: the page lays out with its window again right
  // away, without waiting for a resize, and pending captures fail.
  void OwnerChanged() { FixAgentViewport(); }

 private:
  friend class content::WebContentsUserData<GatewayTab>;

  explicit GatewayTab(content::WebContents* contents)
      : content::WebContentsObserver(contents),
        content::WebContentsUserData<GatewayTab>(*contents),
        sequence_(g_next_tab++),
        tab_id_("tab-" + base::NumberToString(sequence_)) {
    Registry()[sequence_] = this;
  }

  std::optional<std::u16string> GroupTitle() {
    TabStripModel* strip = nullptr;
    int index = TabIndex(&strip);
    if (index == TabStripModel::kNoTab || !strip->SupportsTabGroups()) {
      return std::nullopt;
    }
    std::optional<tab_groups::TabGroupId> group = strip->GetTabGroupForTab(index);
    if (!group) {
      return std::nullopt;
    }
    return strip->group_model()->GetTabGroup(*group)->visual_data()->title();
  }

  int TabIndex(TabStripModel** strip_out) {
    tabs::TabInterface* tab =
        tabs::TabInterface::MaybeGetFromContents(web_contents());
    BrowserWindowInterface* window =
        tab ? tab->GetBrowserWindowInterface() : nullptr;
    TabStripModel* strip = window ? window->GetTabStripModel() : nullptr;
    if (!strip) {
      return TabStripModel::kNoTab;
    }
    *strip_out = strip;
    return strip->GetIndexOfWebContents(web_contents());
  }

  void JoinAgentTabsGroup() {
    TabStripModel* strip = nullptr;
    int index = TabIndex(&strip);
    if (index == TabStripModel::kNoTab || !strip->SupportsTabGroups()) {
      return;
    }
    for (const tab_groups::TabGroupId& id :
         strip->group_model()->ListTabGroups()) {
      if (strip->group_model()->GetTabGroup(id)->visual_data()->title() ==
          kAgentTabsTitle) {
        strip->AddToExistingGroup({index}, id);
        return;
      }
    }
    tab_groups::TabGroupId id = strip->AddToNewGroup({index});
    strip->ChangeTabGroupVisuals(
        id, tab_groups::TabGroupVisualData(kAgentTabsTitle,
                                           tab_groups::TabGroupColorId::kBlue));
  }

  // Every agent page is emulated at 1440x900, DSF 1, wherever it lives: in
  // its dedicated window, hidden in a shared window, or shown in a window the
  // user is looking at. A dedicated window is additionally resized around its
  // content area so the user sees the whole page when the window manager
  // allows it; that is cosmetic and never decides the page layout.
  void FixAgentViewport() {
    viewport_fix_pending_ = false;
    content::WebContents* contents = web_contents();
    if (!IsAgentTab(contents)) {
      // The user took the tab over: it lays out with its window again.
      emulation_.reset();
      reassert_ = false;
      bounds_refused_ = false;
      RunCaptureWaiters(false);
      return;
    }
    TabStripModel* strip = nullptr;
    int index = TabIndex(&strip);
    if (index != TabStripModel::kNoTab && strip->count() == 1) {
      FitDedicatedWindow();
    }
    if (!emulation_) {
      emulation_ = std::make_unique<ViewportEmulation>(
          contents, base::BindRepeating(&GatewayTab::OnEmulationApplied,
                                        weak_factory_.GetWeakPtr()));
      if (!emulation_->attached()) {
        emulation_.reset();
        RunCaptureWaiters(false);
        return;
      }
      reassert_ = true;
    }
    // OnEmulationApplied checks again once the renderer answered.
    if (emulation_->applying()) {
      return;
    }
    if (EmulatedSize() && !reassert_) {
      RunCaptureWaiters(true);
      return;
    }
    // The budget is renewed only by a capture, a tab-strip change or a
    // visibility change, never by the resizes the restores themselves cause.
    if (restores_ >= kMaxViewportCorrections) {
      reassert_ = false;
      RunCaptureWaiters(false);
      return;
    }
    ++restores_;
    reassert_ = false;
    emulation_->Restore();
  }

  bool EmulatedSize() {
    content::RenderWidgetHostView* view =
        web_contents()->GetRenderWidgetHostView();
    return view && view->GetViewBounds().size() == kAgentViewport;
  }

  void OnEmulationApplied(bool applied) {
    if (!applied) {
      // Refused, or the renderer did not answer in time. The next capture,
      // resize, tab-strip or visibility change attaches a fresh session.
      LOG(WARNING) << "openside: agent viewport override was not applied";
      emulation_.reset();
      reassert_ = false;
      RunCaptureWaiters(false);
      return;
    }
    if (reassert_ || !EmulatedSize()) {
      ScheduleViewportFix();
      return;
    }
    RunCaptureWaiters(true);
  }

  void RunCaptureWaiters(bool prepared) {
    std::vector<base::OnceCallback<void(bool)>> waiters =
        std::move(capture_waiters_);
    capture_waiters_.clear();
    for (base::OnceCallback<void(bool)>& waiter : waiters) {
      std::move(waiter).Run(prepared);
    }
  }

  void FitDedicatedWindow() {
    content::WebContents* contents = web_contents();
    gfx::Size view = contents->GetContainerBounds().size();
    if (view == kAgentViewport) {
      viewport_corrections_ = 0;
      return;
    }
    ui::BaseWindow* window = tabs::TabInterface::MaybeGetFromContents(contents)
                                 ->GetBrowserWindowInterface()
                                 ->GetWindow();
    // Before the first layout the view is empty; its first resize retries.
    // Intermediate sizes while a request is in flight are not refusals.
    if (view.IsEmpty() || !window || window->IsMinimized() ||
        window->IsMaximized() || window->IsFullscreen() || bounds_refused_ ||
        bounds_pending_ || viewport_corrections_ >= kMaxViewportCorrections) {
      return;
    }
    ++viewport_corrections_;
    bounds_pending_ = true;
    gfx::Rect bounds = window->GetBounds();
    bounds.set_width(bounds.width() + kAgentViewportWidth - view.width());
    bounds.set_height(bounds.height() + kAgentViewportHeight - view.height());
    window->SetBounds(bounds);
    // A compositor may ignore a client resize without any reply, so the
    // outcome is judged after the configure round trip had time to land.
    base::SequencedTaskRunner::GetCurrentDefault()->PostDelayedTask(
        FROM_HERE,
        base::BindOnce(&GatewayTab::OnBoundsSettled,
                       weak_factory_.GetWeakPtr()),
        kBoundsSettleDelay);
  }

  void OnBoundsSettled() {
    bounds_pending_ = false;
    if (web_contents()->GetContainerBounds().size() != kAgentViewport) {
      bounds_refused_ = true;
    }
  }

  // Moves between windows and other tabs joining or leaving this tab's
  // window change neither its size nor its visibility in every case, so the
  // tab strip is watched as well.
  void WatchTabStrip() {
    tabs::TabInterface* tab =
        tabs::TabInterface::MaybeGetFromContents(web_contents());
    if (!tab) {
      return;
    }
    tab_subscriptions_.push_back(tab->RegisterDidInsert(base::BindRepeating(
        &GatewayTab::OnTabStripChanged, weak_factory_.GetWeakPtr())));
    tab_subscriptions_.push_back(tab->RegisterDidActivate(base::BindRepeating(
        &GatewayTab::OnTabStripChanged, weak_factory_.GetWeakPtr())));
    tab_subscriptions_.push_back(
        tab->RegisterWillDeactivate(base::BindRepeating(
            &GatewayTab::OnTabStripChanged, weak_factory_.GetWeakPtr())));
  }

  void OnTabStripChanged(tabs::TabInterface* tab) {
    restores_ = 0;
    viewport_corrections_ = 0;
    bounds_refused_ = false;
    ReassertViewport();
    ScheduleViewportFix();
  }

  void PrimaryMainFrameWasResized(bool width_changed) override {
    ScheduleViewportFix();
  }

  void OnVisibilityChanged(content::Visibility visibility) override {
    restores_ = 0;
    viewport_corrections_ = 0;
    ScheduleViewportFix();
  }

  void PrimaryPageChanged(content::Page& page) override {
    ++primary_generation_;
    Updated();
  }

  void TitleWasSet(content::NavigationEntry* entry) override { Updated(); }

  void Updated() {
    if (!announced_) {
      return;
    }
    base::DictValue payload;
    payload.Set("tab", Record());
    payload.Set("reason", "navigation");
    Emit("tab.updated", this, std::move(payload));
  }

  void WebContentsDestroyed() override {
    // The WebContents is going away; dropping the runner avoids touching it.
    std::ignore = keep_rendering_.Release();
    emulation_.reset();
    tab_subscriptions_.clear();
    // Pending captures answer tab_not_found once this tab left the registry.
    for (base::OnceCallback<void(bool)>& waiter : capture_waiters_) {
      base::SequencedTaskRunner::GetCurrentDefault()->PostTask(
          FROM_HERE, base::BindOnce(std::move(waiter), false));
    }
    capture_waiters_.clear();
    if (announced_) {
      base::DictValue payload;
      payload.Set("reason", browser_shutdown::HasShutdownStarted()
                                ? "browser_shutdown"
                                : "external");
      Emit("tab.closed", this, std::move(payload));
    }
    Registry().erase(sequence_);
  }

  const uint64_t sequence_;
  const std::string tab_id_;
  bool announced_ = false;
  uint64_t primary_generation_ = 0;
  base::ScopedClosureRunner keep_rendering_;
  uint64_t next_dialog_ = 1;
  std::string open_dialog_;
  bool viewport_fix_pending_ = false;
  int viewport_corrections_ = 0;
  bool bounds_pending_ = false;
  bool bounds_refused_ = false;
  bool reassert_ = false;
  int restores_ = 0;
  std::unique_ptr<ViewportEmulation> emulation_;
  std::vector<base::OnceCallback<void(bool)>> capture_waiters_;
  std::vector<base::CallbackListSubscription> tab_subscriptions_;
  base::WeakPtrFactory<GatewayTab> weak_factory_{this};

  WEB_CONTENTS_USER_DATA_KEY_DECL();
};

WEB_CONTENTS_USER_DATA_KEY_IMPL(GatewayTab);

GatewayTab* FindTab(const std::string& tab_id) {
  for (auto& [sequence, tab] : Registry()) {
    if (tab->tab_id() == tab_id) {
      return tab.get();
    }
  }
  return nullptr;
}

class Connection;

class Gateway : public download::AllDownloadItemNotifier::Observer {
 public:
  // `browsers_dir` and `instance_dir` are the verified descriptors the
  // instance was created through; teardown removes it through them, never by
  // resolving the runtime path again.
  Gateway(std::string instance_id,
          base::ScopedFD browsers_dir,
          base::ScopedFD instance_dir,
          std::string token,
          int listen_fd);

  ~Gateway() override;

  const std::string& instance_id() const { return instance_id_; }
  const std::string& token() const { return token_; }
  const std::string& session_id() const { return session_id_; }
  const std::string& subagent_id() const { return subagent_id_; }
  const base::FilePath& profile_path() const { return profile_path_; }

  void ObserveDownloads(content::BrowserContext* context) {
    content::DownloadManager* manager = context->GetDownloadManager();
    if (manager && !download_notifiers_.contains(manager)) {
      download_notifiers_[manager] =
          std::make_unique<download::AllDownloadItemNotifier>(manager, this);
    }
    const GURL ui = DaemonUiUrl();
    auto* profile = Profile::FromBrowserContext(context);
    if (profile_path_.empty() && !profile->IsOffTheRecord()) {
      profile_path_ = profile->GetPath();
    }
    auto* service = ui.is_empty() || profile->IsOffTheRecord() ||
                            profile->GetPath() != profile_path_
                        ? nullptr
                        : TemplateURLServiceFactory::GetForProfile(profile);
    if (service && !ui_subscriptions_.contains(context)) {
      if (service->loaded()) {
        RegisterUiKeywords(service, ui);
        ui_subscriptions_.try_emplace(context);
      } else {
        ui_subscriptions_.emplace(
            context, service->RegisterOnLoadedCallback(base::BindOnce(
                         &RegisterUiKeywords, base::Unretained(service), ui)));
        service->Load();
      }
    }
  }

  void Emit(std::string_view type, GatewayTab* tab, base::DictValue payload);

  void DropConnection(Connection* connection);

 private:
  void OnAcceptable();

  void OnDownloadCreated(content::DownloadManager* manager,
                         download::DownloadItem* item) override {
    base::DictValue payload;
    payload.Set("download_id", item->GetGuid());
    // v1 never carries file names or paths over the wire.
    payload.Set("suggested_filename_redacted", true);
    content::WebContents* contents =
        content::DownloadItemUtils::GetWebContents(item);
    Emit("download.started", contents ? GatewayTab::FromWebContents(contents)
                                      : nullptr,
         std::move(payload));
    OnDownloadUpdated(manager, item);
  }

  void OnDownloadUpdated(content::DownloadManager* manager,
                         download::DownloadItem* item) override {
    const char* state = nullptr;
    switch (item->GetState()) {
      case download::DownloadItem::COMPLETE:
        state = "complete";
        break;
      case download::DownloadItem::CANCELLED:
        state = "cancelled";
        break;
      case download::DownloadItem::INTERRUPTED:
        state = "interrupted";
        break;
      default:
        return;
    }
    // Terminal states are reported exactly once per download.
    if (!terminal_downloads_.insert(item->GetGuid()).second) {
      return;
    }
    terminal_order_.push_back(item->GetGuid());
    if (terminal_order_.size() > kMaxTerminalDownloads) {
      terminal_downloads_.erase(terminal_order_.front());
      terminal_order_.pop_front();
    }
    base::DictValue payload;
    payload.Set("download_id", item->GetGuid());
    payload.Set("state", state);
    payload.Set("received_bytes",
                Count(static_cast<uint64_t>(
                    std::max<int64_t>(0, item->GetReceivedBytes()))));
    int64_t total = item->GetTotalBytes();
    payload.Set("total_bytes", total > 0 ? Count(static_cast<uint64_t>(total))
                                         : base::Value());
    content::WebContents* contents =
        content::DownloadItemUtils::GetWebContents(item);
    Emit("download.updated",
         contents ? GatewayTab::FromWebContents(contents) : nullptr,
         std::move(payload));
  }

  void OnDownloadRemoved(content::DownloadManager* manager,
                         download::DownloadItem* item) override {
    if (terminal_downloads_.erase(item->GetGuid())) {
      std::erase(terminal_order_, item->GetGuid());
    }
  }

  const std::string instance_id_;
  const base::ScopedFD browsers_dir_;
  const base::ScopedFD instance_dir_;
  const std::string token_;
  const int listen_fd_;
  const std::string session_id_;
  const std::string subagent_id_;
  base::FilePath profile_path_;
  std::unique_ptr<base::FileDescriptorWatcher::Controller> accept_watch_;
  std::unique_ptr<Connection> connection_;
  std::map<content::DownloadManager*,
           std::unique_ptr<download::AllDownloadItemNotifier>>
      download_notifiers_;
  std::map<content::BrowserContext*, base::CallbackListSubscription>
      ui_subscriptions_;
  std::set<std::string> terminal_downloads_;
  std::deque<std::string> terminal_order_;
};

// Encodes a capture result off the UI thread; touches no browser objects.
struct EncodedCapture {
  std::optional<std::string> frame;
  const char* code = nullptr;
  const char* message = nullptr;
};

EncodedCapture EncodeCapture(const std::string& id,
                             const std::string& tab_id,
                             SkBitmap bitmap) {
  EncodedCapture encoded;
  if (bitmap.width() != kAgentViewportWidth ||
      bitmap.height() != kAgentViewportHeight) {
    encoded.code = "capture_failed";
    encoded.message = "capture is not 1440x900";
    return encoded;
  }
  std::optional<std::vector<uint8_t>> png =
      gfx::PNGCodec::EncodeBGRASkBitmap(bitmap,
                                        /*discard_transparency=*/true);
  if (!png) {
    encoded.code = "capture_failed";
    encoded.message = "encoding failed";
    return encoded;
  }
  if (png->size() > kMaxPngBytes) {
    encoded.code = "resource_exhausted";
    encoded.message = "image too large";
    return encoded;
  }
  base::DictValue payload;
  payload.Set("tab_id", tab_id);
  payload.Set("format", "png");
  payload.Set("width", bitmap.width());
  payload.Set("height", bitmap.height());
  payload.Set("data_base64", base::Base64Encode(*png));
  base::DictValue result;
  result.Set("v", 1);
  result.Set("kind", "result");
  result.Set("id", id);
  result.Set("type", "screenshot.capture");
  result.Set("payload", std::move(payload));
  encoded.frame = ToJson(result);
  if (!encoded.frame) {
    encoded.code = "internal";
    encoded.message = "encoding failed";
  }
  return encoded;
}

// One authenticated control connection. All browser-state work runs on the UI
// sequence; the IO thread only signals fd readiness, and PNG/base64 encoding
// runs on the thread pool. Input is read and handled in bounded slices so a
// fast or hostile peer can neither grow memory nor monopolize the UI thread.
class Connection {
 public:
  Connection(Gateway* gateway, int fd) : gateway_(gateway), fd_(fd) {
    ArmRead();
    handshake_timer_.Start(FROM_HERE, kHandshakeTimeout,
                           base::BindOnce(&Connection::Close,
                                          weak_factory_.GetWeakPtr()));
  }

  ~Connection() {
    read_watch_.reset();
    write_watch_.reset();
    close(fd_);
  }

  bool authenticated() const { return authenticated_ && !closing_; }

  void SendEvent(std::string_view type,
                 GatewayTab* tab,
                 base::DictValue payload) {
    if (next_event_seq_ > kMaxSequence) {
      Fail("resource_exhausted", "event sequence exhausted");
      return;
    }
    base::DictValue event;
    event.Set("v", 1);
    event.Set("kind", "event");
    event.Set("type", type);
    event.Set("event_seq", Count(next_event_seq_++));
    event.Set("instance_id", gateway_->instance_id());
    base::Value session;
    base::Value subagent;
    if (tab) {
      event.Set("tab_id", tab->tab_id());
      const TabOwnership* ownership =
          TabOwnership::FromWebContents(tab->web_contents());
      if (ownership && ownership->owner() == TabOwner::kAgent) {
        session = Nullable(ownership->session_id());
        subagent = Nullable(ownership->subagent_id());
      }
    }
    event.Set("session_id", std::move(session));
    event.Set("subagent_id", std::move(subagent));
    event.Set("payload", std::move(payload));
    Send(std::move(event));
  }

  void Close() {
    if (closing_) {
      return;
    }
    closing_ = true;
    handshake_timer_.Stop();
    read_watch_.reset();
    write_watch_.reset();
    shutdown(fd_, SHUT_RDWR);
    base::SequencedTaskRunner::GetCurrentDefault()->PostTask(
        FROM_HERE, base::BindOnce(&Connection::Drop, weak_factory_.GetWeakPtr()));
  }

 private:
  void Drop() { gateway_->DropConnection(this); }

  bool Stopping() const { return closing_ || close_after_flush_; }

  void ArmRead() {
    if (!read_watch_ && !Stopping()) {
      read_watch_ = base::FileDescriptorWatcher::WatchReadable(
          fd_, base::BindRepeating(&Connection::OnReadable,
                                   weak_factory_.GetWeakPtr()));
    }
  }

  size_t FrameLimit() const {
    return authenticated_ ? kMaxRequestFrameBytes : kMaxHelloFrameBytes;
  }

  // Rejects a declared length before its body is accepted.
  bool HeadFrameLengthValid(size_t offset) {
    const auto* bytes = reinterpret_cast<const uint8_t*>(input_.data() + offset);
    uint32_t length =
        base::U32FromBigEndian(std::span<const uint8_t, 4>(bytes, 4u));
    if (length == 0) {
      Fail("protocol_error", "invalid frame length");
      return false;
    }
    if (length > FrameLimit()) {
      Fail("frame_too_large", "invalid frame length");
      return false;
    }
    return true;
  }

  void OnReadable() {
    std::array<char, 64 * 1024> buffer;
    size_t budget = kReadBudgetBytes;
    while (!Stopping() && budget > 0 && input_.size() < kMaxInputBytes) {
      size_t want =
          std::min({buffer.size(), budget, kMaxInputBytes - input_.size()});
      ssize_t n = read(fd_, buffer.data(), want);
      if (n > 0) {
        input_.append(buffer.data(), static_cast<size_t>(n));
        budget -= static_cast<size_t>(n);
        if (input_.size() >= 4 && !HeadFrameLengthValid(0)) {
          return;
        }
        continue;
      }
      if (n < 0 && errno == EINTR) {
        continue;
      }
      if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) {
        break;
      }
      // EOF, including EOF in the middle of a frame, ends the connection.
      Close();
      return;
    }
    ProcessFrames();
  }

  bool HasCompleteFrame() const {
    if (input_.size() < 4) {
      return false;
    }
    const auto* bytes = reinterpret_cast<const uint8_t*>(input_.data());
    uint32_t length =
        base::U32FromBigEndian(std::span<const uint8_t, 4>(bytes, 4u));
    return input_.size() - 4 >= length;
  }

  // Handles at most kFramesPerTask frames, then yields. Reading pauses while
  // complete frames are still buffered, which bounds input_ at one frame, and
  // while kPauseQueuedFrames replies wait for a slow reader; Flush resumes once
  // they drained. The pause counts frames only: it keeps pipelined small
  // replies from reaching the hard limits, not large ones.
  void ProcessFrames() {
    size_t offset = 0;
    int handled = 0;
    while (!Stopping() && handled < kFramesPerTask && !Backlogged() &&
           input_.size() - offset >= 4) {
      if (!HeadFrameLengthValid(offset)) {
        return;
      }
      const auto* bytes =
          reinterpret_cast<const uint8_t*>(input_.data() + offset);
      uint32_t length =
          base::U32FromBigEndian(std::span<const uint8_t, 4>(bytes, 4u));
      if (input_.size() - offset - 4 < length) {
        break;
      }
      std::string_view payload(input_.data() + offset + 4, length);
      HandleFrame(payload);
      offset += 4 + length;
      ++handled;
    }
    if (closing_) {
      return;
    }
    input_.erase(0, offset);
    if (Stopping()) {
      return;
    }
    if (Backlogged()) {
      read_watch_.reset();
      paused_ = true;
      return;
    }
    if (HasCompleteFrame()) {
      read_watch_.reset();
      base::SequencedTaskRunner::GetCurrentDefault()->PostTask(
          FROM_HERE, base::BindOnce(&Connection::ProcessFrames,
                                    weak_factory_.GetWeakPtr()));
      return;
    }
    ArmRead();
  }

  bool Backlogged() const { return output_.size() >= kPauseQueuedFrames; }

  void HandleFrame(std::string_view frame) {
    std::optional<base::Value> parsed =
        base::JSONReader::Read(frame, base::JSON_PARSE_RFC);
    if (!parsed || !parsed->is_dict()) {
      Fail("protocol_error", "frame is not a JSON object");
      return;
    }
    base::DictValue& message = parsed->GetDict();
    if (!authenticated_) {
      Handshake(message);
      return;
    }
    HandleRequest(message);
  }

  void Handshake(const base::DictValue& hello) {
    const std::string* kind = hello.FindString("kind");
    const std::string* protocol = hello.FindString("protocol");
    const base::ListValue* versions = hello.FindList("versions");
    const std::string* instance = hello.FindString("instance_id");
    const std::string* token = hello.FindString("token");
    if (!kind || *kind != "hello" || !protocol || *protocol != kProtocol ||
        !versions || !instance || !token) {
      Fail("protocol_error", "expected hello");
      return;
    }
    if (*instance != gateway_->instance_id() ||
        !ConstantTimeEquals(*token, gateway_->token())) {
      Fail("auth_failed", "authentication failed");
      return;
    }
    bool has_v1 = false;
    for (const base::Value& version : *versions) {
      has_v1 = has_v1 || (version.is_int() && version.GetInt() == 1);
    }
    if (!has_v1) {
      Fail("version_mismatch", "no common protocol version");
      return;
    }
    handshake_timer_.Stop();
    authenticated_ = true;
    base::DictValue welcome;
    welcome.Set("kind", "welcome");
    welcome.Set("v", 1);
    welcome.Set("instance_id", gateway_->instance_id());
    base::ListValue capabilities;
    for (const char* capability : {"tab_lifecycle", "focus", "hidden_screenshot",
                                   "popup", "dialog", "download",
                                   "browser_status", "native_autofill"}) {
      capabilities.Append(capability);
    }
    welcome.Set("capabilities", std::move(capabilities));
    Send(std::move(welcome));
  }

  // False when the id was used within the recent window on this connection.
  bool RememberRequestId(const std::string& id) {
    if (!seen_ids_.insert(id).second) {
      return false;
    }
    seen_order_.push_back(id);
    if (seen_order_.size() > kRequestIdWindow) {
      seen_ids_.erase(seen_order_.front());
      seen_order_.pop_front();
    }
    return true;
  }

  void HandleRequest(const base::DictValue& request) {
    const std::string* id = request.FindString("id");
    const std::string* type = request.FindString("type");
    const std::string* kind = request.FindString("kind");
    std::optional<int> version = request.FindInt("v");
    const base::Value* session = request.Find("session_id");
    const base::Value* subagent = request.Find("subagent_id");
    const base::DictValue* payload = request.FindDict("payload");
    if (!id || id->empty() || id->size() > kMaxRequestIdBytes || !type) {
      Fail("protocol_error", "request requires id and type");
      return;
    }
    if (!kind || *kind != "request" || version != 1 || !session ||
        !session->is_string() || !subagent ||
        !(subagent->is_none() || subagent->is_string()) || !payload) {
      Error(*id, *type, "invalid_request", "malformed request envelope");
      return;
    }
    if (!RememberRequestId(*id)) {
      Error(*id, *type, "invalid_request", "request id reused");
      return;
    }
    const std::string requested_subagent =
        subagent->is_string() ? subagent->GetString() : std::string();
    if (gateway_->session_id().empty() ||
        session->GetString() != gateway_->session_id() ||
        requested_subagent != gateway_->subagent_id() ||
        (subagent->is_string() && requested_subagent.empty())) {
      Error(*id, *type, "scope_not_found", "unknown session scope");
      return;
    }
    if (*type == "tabs.list") {
      TabsList(*id);
    } else if (*type == "browser.status") {
      BrowserStatus(*id);
    } else if (*type == "autofill.fields") {
      AutofillFields(*id, *payload);
    } else if (*type == "autofill.fill") {
      AutofillFill(*id, *payload);
    } else if (*type == "focus.request") {
      FocusRequest(*id, *payload);
    } else if (*type == "screenshot.capture") {
      Screenshot(*id, *payload);
    } else if (*type == "tab.set_label") {
      SetLabel(*id, *payload);
    } else if (*type == "tab.open" || *type == "tab.close" ||
               *type == "dialog.respond" || *type == "download.cancel") {
      Error(*id, *type, "unsupported", "request not implemented by this build");
    } else {
      Error(*id, *type, "invalid_request", "unknown request type");
    }
  }

  // Ownership requires the exact launch scope; owner:user after a takeover
  // or another scope's tab is denied.
  TabOwnership* OwnedTab(GatewayTab* tab) {
    TabOwnership* ownership = TabOwnership::FromWebContents(tab->web_contents());
    if (!ownership || ownership->owner() != TabOwner::kAgent ||
        ownership->opened_by() != OpenedBy::kAgent ||
        ownership->session_id() != gateway_->session_id() ||
        ownership->subagent_id() != gateway_->subagent_id()) {
      return nullptr;
    }
    return ownership;
  }

  struct FillHandle {
    std::string tab_id;
    content::WeakDocumentPtr document;
    blink::DocumentToken renderer_document;
    url::Origin origin;
    uint64_t primary_generation;
    autofill::FieldRendererId field;
    base::TimeTicks expires;
  };

  void AutofillFields(const std::string& id, const base::DictValue& payload) {
    const std::string* tab_id = payload.FindString("tab_id");
    GatewayTab* tab = tab_id ? FindTab(*tab_id) : nullptr;
    if (!tab || !OwnedTab(tab)) {
      Error(id, "autofill.fields", "ownership_denied", "agent tab required");
      return;
    }
    auto* frame = tab->web_contents()->GetPrimaryMainFrame();
    auto* driver =
        password_manager::ContentPasswordManagerDriver::GetForRenderFrameHost(
            frame);
    if (!driver || !frame->IsRenderFrameLive() ||
        frame->GetLastCommittedOrigin().opaque()) {
      Error(id, "autofill.fields", "tab_asleep", "autofill frame unavailable");
      return;
    }
    const auto node = payload.FindInt("backend_node_id");
    if (!node || *node <= 0) {
      Error(id, "autofill.fields", "invalid_request", "field identity required");
      return;
    }
    driver->ExtractFieldForOpenside(
        autofill::FieldRendererId(*node),
        base::BindOnce(&Connection::ExtractedFields, weak_factory_.GetWeakPtr(),
                       id, *tab_id, frame->GetWeakDocumentPtr(),
                       tab->primary_generation()));
  }

  void AddFillField(GatewayTab* tab,
                    content::RenderFrameHost* frame,
                    const blink::DocumentToken& renderer_document,
                    const url::Origin& origin,
                    const autofill::FormFieldData& field,
                    base::ListValue& fields) {
    if ((!field.IsTextInputElement() && !field.IsPasswordInputElement()) ||
        fill_handles_.size() >= 256) {
      return;
    }
    std::array<uint8_t, 16> random;
    base::RandBytes(random);
    const std::string handle = base::HexEncode(random);
    fill_handles_.emplace(
        handle, FillHandle{tab->tab_id(), frame->GetWeakDocumentPtr(),
                           renderer_document, origin, tab->primary_generation(),
                           field.renderer_id(),
                           base::TimeTicks::Now() + base::Seconds(60)});
    base::DictValue record;
    record.Set("handle", handle);
    record.Set("name", base::UTF16ToUTF8(field.name()));
    record.Set("origin", origin.Serialize());
    record.Set("backend_node_id", Count(field.renderer_id().value()));
    record.Set("kind", field.IsPasswordInputElement() ? "password" : "text");
    fields.Append(std::move(record));
  }

  void ExtractedFields(
      const std::string& id,
      const std::string& tab_id,
      content::WeakDocumentPtr document,
      uint64_t primary_generation,
      const std::optional<autofill::FormData>& form,
      const std::optional<blink::DocumentToken>& renderer_document,
      const url::Origin& renderer_origin) {
    GatewayTab* tab = FindTab(tab_id);
    auto* frame = document.AsRenderFrameHostIfValid();
    if (!tab || !OwnedTab(tab) || !frame ||
        frame != tab->web_contents()->GetPrimaryMainFrame() || !form ||
        !renderer_document || renderer_origin.opaque() ||
        renderer_origin != frame->GetLastCommittedOrigin() ||
        primary_generation != tab->primary_generation()) {
      Error(id, "autofill.fields", "invalid_request", "field form unavailable");
      return;
    }
    fill_handles_.clear();
    base::ListValue fields;
    for (const auto& field : form->fields()) {
      AddFillField(tab, frame, *renderer_document, renderer_origin, field, fields);
    }
    base::DictValue result;
    result.Set("fields", std::move(fields));
    Result(id, "autofill.fields", std::move(result));
  }

  void AutofillFill(const std::string& id, const base::DictValue& payload) {
    const std::string* tab_id = payload.FindString("tab_id");
    const std::string* handle = payload.FindString("handle");
    const std::string* origin = payload.FindString("origin");
    const std::string* value = payload.FindString("value");
    if (!tab_id || !handle || !origin || !value || value->size() > 65536) {
      Error(id, "autofill.fill", "invalid_request", "invalid fill request");
      return;
    }
    GatewayTab* tab = FindTab(*tab_id);
    if (!tab || !OwnedTab(tab)) {
      Error(id, "autofill.fill", "ownership_denied", "agent tab required");
      return;
    }
    auto found = fill_handles_.find(*handle);
    if (found == fill_handles_.end() || found->second.tab_id != *tab_id) {
      Error(id, "autofill.fill", "invalid_request", "unknown field handle");
      return;
    }
    FillHandle binding = std::move(found->second);
    fill_handles_.erase(found);
    auto* frame = binding.document.AsRenderFrameHostIfValid();
    if (!frame || frame != tab->web_contents()->GetPrimaryMainFrame() ||
        !frame->IsRenderFrameLive() ||
        binding.primary_generation != tab->primary_generation() ||
        binding.expires <= base::TimeTicks::Now()) {
      Error(id, "autofill.fill", "invalid_request", "stale field handle");
      return;
    }
    if (frame->GetLastCommittedOrigin().opaque() ||
        binding.origin != frame->GetLastCommittedOrigin() ||
        binding.origin.Serialize() != *origin) {
      Error(id, "autofill.fill", "policy_denied", "origin mismatch");
      return;
    }
    auto* driver =
        password_manager::ContentPasswordManagerDriver::GetForRenderFrameHost(
            frame);
    if (!driver) {
      Error(id, "autofill.fill", "unsupported", "autofill driver unavailable");
      return;
    }
    // The only value-bearing hop is PasswordAutofillAgent Mojo, never DevTools.
    driver->FillFieldForOpenside(
        binding.field, base::UTF8ToUTF16(*value), binding.renderer_document,
        binding.origin,
        base::BindOnce(
            [](base::WeakPtr<Connection> connection, std::string request_id,
               bool filled) {
              if (connection) {
                base::DictValue result;
                result.Set("filled", filled);
                connection->Result(request_id, "autofill.fill",
                                   std::move(result));
              }
            },
            weak_factory_.GetWeakPtr(), id));
  }

  void TabsList(const std::string& id) {
    base::ListValue tabs;
    for (auto& [sequence, tab] : Registry()) {
      tabs.Append(tab->Record());
    }
    base::DictValue payload;
    payload.Set("tabs", std::move(tabs));
    // Events are emitted synchronously on this sequence, so the snapshot
    // covers every event sent before this result.
    payload.Set("through_event_seq", Count(next_event_seq_ - 1));
    Result(id, "tabs.list", std::move(payload));
  }

  void BrowserStatus(const std::string& id) {
    int automation_infobars = 0;
    for (auto& [sequence, tab] : Registry()) {
      auto* manager =
          infobars::ContentInfoBarManager::FromWebContents(tab->web_contents());
      if (!manager) {
        continue;
      }
      for (const auto& infobar : manager->infobars()) {
        automation_infobars +=
            infobar->GetIdentifier() ==
            infobars::InfoBarDelegate::AUTOMATION_INFOBAR_DELEGATE;
      }
    }
    bool vertical_tabs = false;
    bool bookmarks_side_panel = false;
    base::DictValue ui;
    ui.Set("ask", false);
    ui.Set("routines", false);
    const GURL daemon_ui = DaemonUiUrl();
    ForEachCurrentBrowserWindowInterfaceOrderedByActivation(
        [&](BrowserWindowInterface* window) {
          if (window->GetProfile()->IsOffTheRecord() ||
              window->GetProfile()->GetPath() != gateway_->profile_path()) {
            return true;
          }
          vertical_tabs |= window->GetProfile()->GetPrefs()->GetBoolean(
              prefs::kVerticalTabsEnabled);
          auto* registry = SidePanelRegistry::From(window);
          bookmarks_side_panel |=
              registry && registry->GetEntryForKey(
                              SidePanelEntry::Key(SidePanelEntry::Id::kBookmarks));
          auto* service =
              TemplateURLServiceFactory::GetForProfile(window->GetProfile());
          if (!daemon_ui.is_empty() && service && service->loaded()) {
            for (const auto& [keyword, suffix, key] :
                 {std::tuple{u"ask", "?q={searchTerms}", "ask"},
                  std::tuple{u"routines", "routines?q={searchTerms}",
                             "routines"}}) {
              const auto* entry = service->GetTemplateURLForKeyword(keyword);
              ui.Set(key, entry && entry->url() == daemon_ui.spec() + suffix);
            }
          }
          return true;
        });
    base::DictValue ux;
    ux.Set("vertical_tabs", vertical_tabs);
    ux.Set("split_view",
           base::FeatureList::IsEnabled(tabs::kSplitViewHorizontal));
    ux.Set("bookmarks_side_panel", bookmarks_side_panel);
    base::DictValue payload;
    payload.Set("automation_infobar_count", automation_infobars);
    payload.Set("ux", std::move(ux));
    payload.Set("ui", std::move(ui));
    Result(id, "browser.status", std::move(payload));
  }

  void FocusRequest(const std::string& id, const base::DictValue& payload) {
    const std::string* tab_id = payload.FindString("tab_id");
    const base::Value* grant = payload.Find("grant_id");
    if (!tab_id || !grant || !(grant->is_none() || grant->is_string())) {
      Error(id, "focus.request", "invalid_request", "tab_id and grant_id required");
      return;
    }
    GatewayTab* tab = FindTab(*tab_id);
    if (!tab) {
      Error(id, "focus.request", "tab_not_found", "unknown tab");
      return;
    }
    if (grant->is_string()) {
      // No native UI issues grants in this build, so any grant is invalid.
      Error(id, "focus.request", "focus_denied", "unknown or expired grant");
      return;
    }
    FocusDecision decision =
        DecideFocus(FocusActor::kAgent, gateway_->session_id(),
                    gateway_->subagent_id(), FocusContext());
    bool allowed = decision == FocusDecision::kAllow;
    base::DictValue result;
    result.Set("allowed", allowed);
    result.Set("reason", FocusReason(decision));
    result.Set("owner", allowed ? "agent" : "user");
    base::DictValue decided = result.Clone();
    decided.Set("request_id", id);
    SendEvent("focus.decided", tab, std::move(decided));
    Result(id, "focus.request", std::move(result));
  }

  void SetLabel(const std::string& id, const base::DictValue& payload) {
    const std::string* tab_id = payload.FindString("tab_id");
    const std::string* label = payload.FindString("label");
    if (!tab_id || !label || label->size() > 256) {
      Error(id, "tab.set_label", "invalid_request", "tab_id and label required");
      return;
    }
    GatewayTab* tab = FindTab(*tab_id);
    if (!tab) {
      Error(id, "tab.set_label", "tab_not_found", "unknown tab");
      return;
    }
    TabOwnership* ownership = OwnedTab(tab);
    if (!ownership) {
      Error(id, "tab.set_label", "ownership_denied", "tab not owned by scope");
      return;
    }
    ownership->set_label(*label);
    base::DictValue updated;
    updated.Set("tab", tab->Record());
    updated.Set("reason", "label");
    SendEvent("tab.updated", tab, std::move(updated));
    base::DictValue result;
    result.Set("tab", tab->Record());
    Result(id, "tab.set_label", std::move(result));
  }

  // Captures through the compositor surface; never selects the tab, raises
  // a window, or falls back to a foreground capture.
  void Screenshot(const std::string& id, const base::DictValue& payload) {
    const std::string* tab_id = payload.FindString("tab_id");
    const std::string* format = payload.FindString("format");
    if (!tab_id || !format || *format != "png") {
      Error(id, "screenshot.capture", "invalid_request", "png only");
      return;
    }
    GatewayTab* tab = FindTab(*tab_id);
    if (!tab) {
      Error(id, "screenshot.capture", "tab_not_found", "unknown tab");
      return;
    }
    if (!OwnedTab(tab)) {
      Error(id, "screenshot.capture", "ownership_denied", "tab not owned by scope");
      return;
    }
    if (tab->web_contents()->WasDiscarded()) {
      Error(id, "screenshot.capture", "tab_asleep", "tab is asleep");
      return;
    }
    if (captures_.size() >= kMaxCapturesInFlight) {
      Error(id, "screenshot.capture", "resource_exhausted",
            "too many captures in flight");
      return;
    }
    // Each capture holds a slot until exactly one of its stages or its
    // deadline answers; a stage arriving after that finds no entry.
    const uint64_t capture = ++next_capture_;
    captures_.emplace(capture, PendingCapture{id, *tab_id,
                                              base::TimeTicks::Now() +
                                                  kCaptureTimeout});
    base::SequencedTaskRunner::GetCurrentDefault()->PostDelayedTask(
        FROM_HERE,
        base::BindOnce(&Connection::ExpireCapture, weak_factory_.GetWeakPtr(),
                       capture),
        kCaptureTimeout);
    tab->PrepareCapture(base::BindOnce(&Connection::Capture,
                                       weak_factory_.GetWeakPtr(), capture));
  }

  void ExpireCapture(uint64_t capture) {
    FailCapture(capture, "timeout", "capture did not finish in time");
  }

  // The scope may have lost the tab while the capture waited.
  GatewayTab* CapturedTab(uint64_t capture) {
    auto pending = captures_.find(capture);
    if (pending == captures_.end()) {
      return nullptr;
    }
    GatewayTab* tab = FindTab(pending->second.tab_id);
    if (!tab) {
      FailCapture(capture, "tab_not_found", "unknown tab");
      return nullptr;
    }
    if (!OwnedTab(tab)) {
      FailCapture(capture, "ownership_denied", "tab not owned by scope");
      return nullptr;
    }
    return tab;
  }

  void Capture(uint64_t capture, bool prepared) {
    GatewayTab* tab = CapturedTab(capture);
    if (!tab) {
      return;
    }
    if (!prepared) {
      FailCapture(capture, "surface_unavailable",
                  "the 1440x900 viewport was not applied");
      return;
    }
    content::RenderWidgetHostView* view =
        tab->web_contents()->GetRenderWidgetHostView();
    if (!view || !view->IsSurfaceAvailableForCopy()) {
      FailCapture(capture, "surface_unavailable", "no surface");
      return;
    }
    // The capture reflects the page's real layout, never a rescaled image of
    // another viewport.
    if (view->GetViewBounds().size() != kAgentViewport) {
      tab->ScheduleViewportFix();
      FailCapture(capture, "surface_unavailable", "viewport is not 1440x900");
      return;
    }
    // Output size equals the viewport in DIPs, so device scale factors other
    // than 1 still yield one pixel per CSS pixel.
    view->CopyFromSurface(
        gfx::Rect(), kAgentViewport,
        std::max(captures_.at(capture).deadline - base::TimeTicks::Now(),
                 base::TimeDelta()),
        base::BindPostTaskToCurrentDefault(base::BindOnce(
            &Connection::OnCaptured, weak_factory_.GetWeakPtr(), capture)));
  }

  void OnCaptured(uint64_t capture,
                  const content::CopyFromSurfaceResult& result) {
    if (!CapturedTab(capture)) {
      return;
    }
    if (!result.has_value() || result->bitmap.drawsNothing()) {
      FailCapture(capture, "capture_failed", "capture failed");
      return;
    }
    const PendingCapture& pending = captures_.at(capture);
    base::ThreadPool::PostTaskAndReplyWithResult(
        FROM_HERE, {base::TaskPriority::USER_VISIBLE},
        base::BindOnce(&EncodeCapture, pending.id, pending.tab_id,
                       result->bitmap),
        base::BindOnce(&Connection::OnEncoded, weak_factory_.GetWeakPtr(),
                       capture));
  }

  void OnEncoded(uint64_t capture, EncodedCapture encoded) {
    // A handoff while encoding discards the image.
    if (!CapturedTab(capture)) {
      return;
    }
    if (!encoded.frame) {
      FailCapture(capture, encoded.code, encoded.message);
      return;
    }
    captures_.erase(capture);
    SendFrame(std::move(*encoded.frame));
  }

  void FailCapture(uint64_t capture,
                   std::string_view code,
                   std::string_view message) {
    auto pending = captures_.find(capture);
    if (pending == captures_.end()) {
      return;
    }
    const std::string id = std::move(pending->second.id);
    captures_.erase(pending);
    Error(id, "screenshot.capture", code, message);
  }

  void Result(const std::string& id,
              std::string_view type,
              base::DictValue payload) {
    base::DictValue result;
    result.Set("v", 1);
    result.Set("kind", "result");
    result.Set("id", id);
    result.Set("type", type);
    result.Set("payload", std::move(payload));
    Send(std::move(result));
  }

  void Error(const std::string& id,
             std::string_view type,
             std::string_view code,
             std::string_view message) {
    base::DictValue error;
    error.Set("v", 1);
    error.Set("kind", "error");
    error.Set("id", id);
    error.Set("type", type);
    error.Set("code", code);
    error.Set("message", message);
    error.Set("details", base::DictValue());
    Send(std::move(error));
  }

  // Connection-level failures: report a fixed message, then close.
  void Fail(std::string_view code, std::string_view message) {
    if (Stopping()) {
      return;
    }
    base::DictValue error;
    error.Set("v", 1);
    error.Set("kind", "error");
    error.Set("id", base::Value());
    error.Set("type", base::Value());
    error.Set("code", code);
    error.Set("message", message);
    error.Set("details", base::DictValue());
    Send(std::move(error));
    close_after_flush_ = true;
    read_watch_.reset();
    if (output_.empty()) {
      Close();
    }
  }

  void Send(base::DictValue message) {
    if (closing_) {
      return;
    }
    std::optional<std::string> json = ToJson(message);
    if (!json) {
      Close();
      return;
    }
    SendFrame(std::move(*json));
  }

  void SendFrame(std::string json) {
    if (closing_) {
      return;
    }
    if (json.size() > kMaxFrameBytes) {
      Close();
      return;
    }
    std::array<uint8_t, 4> prefix =
        base::U32ToBigEndian(static_cast<uint32_t>(json.size()));
    std::string frame(reinterpret_cast<const char*>(prefix.data()),
                      prefix.size());
    frame += json;
    queued_bytes_ += frame.size();
    output_.push_back(std::move(frame));
    // Hard limits, whatever the pause did: events, replies of requests that
    // were already running (captures, autofill) and a few large replies such
    // as screenshots can still exceed them, and the reader is disconnected.
    if (output_.size() > kMaxQueuedFrames || queued_bytes_ > kMaxQueuedBytes) {
      Close();
      return;
    }
    Flush();
  }

  void Flush() {
    bool progressed = false;
    while (!closing_ && !output_.empty()) {
      const std::string& front = output_.front();
      ssize_t n = send(fd_, front.data() + front_offset_,
                       front.size() - front_offset_, MSG_NOSIGNAL);
      if (n < 0 && errno == EINTR) {
        continue;
      }
      if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) {
        if (!write_watch_) {
          write_watch_ = base::FileDescriptorWatcher::WatchWritable(
              fd_, base::BindRepeating(&Connection::OnWritable,
                                       weak_factory_.GetWeakPtr()));
        }
        // A client that reads nothing for this long is disconnected; one
        // that reads slowly keeps restarting the timer.
        if (progressed || !stalled_.IsRunning()) {
          stalled_.Start(FROM_HERE, kStalledReaderTimeout,
                         base::BindOnce(&Connection::Close,
                                        base::Unretained(this)));
        }
        Resume();
        return;
      }
      if (n <= 0) {
        Close();
        return;
      }
      progressed = true;
      front_offset_ += static_cast<size_t>(n);
      if (front_offset_ == front.size()) {
        queued_bytes_ -= front.size();
        output_.pop_front();
        front_offset_ = 0;
      }
    }
    write_watch_.reset();
    stalled_.Stop();
    if (close_after_flush_) {
      Close();
      return;
    }
    Resume();
  }

  void Resume() {
    if (!paused_ || Backlogged() || Stopping()) {
      return;
    }
    paused_ = false;
    base::SequencedTaskRunner::GetCurrentDefault()->PostTask(
        FROM_HERE,
        base::BindOnce(&Connection::ProcessFrames, weak_factory_.GetWeakPtr()));
  }

  void OnWritable() { Flush(); }

  raw_ptr<Gateway> gateway_;
  const int fd_;
  bool authenticated_ = false;
  bool closing_ = false;
  bool close_after_flush_ = false;
  bool paused_ = false;
  uint64_t next_event_seq_ = 1;
  struct PendingCapture {
    std::string id;
    std::string tab_id;
    base::TimeTicks deadline;
  };
  std::map<uint64_t, PendingCapture> captures_;
  uint64_t next_capture_ = 0;
  std::string input_;
  std::deque<std::string> output_;
  size_t front_offset_ = 0;
  size_t queued_bytes_ = 0;
  std::set<std::string> seen_ids_;
  std::deque<std::string> seen_order_;
  std::map<std::string, FillHandle> fill_handles_;
  base::OneShotTimer handshake_timer_;
  base::OneShotTimer stalled_;
  std::unique_ptr<base::FileDescriptorWatcher::Controller> read_watch_;
  std::unique_ptr<base::FileDescriptorWatcher::Controller> write_watch_;
  base::WeakPtrFactory<Connection> weak_factory_{this};
};

Gateway::Gateway(std::string instance_id,
                 base::ScopedFD browsers_dir,
                 base::ScopedFD instance_dir,
                 std::string token,
                 int listen_fd)
    : instance_id_(std::move(instance_id)),
      browsers_dir_(std::move(browsers_dir)),
      instance_dir_(std::move(instance_dir)),
      token_(std::move(token)),
      listen_fd_(listen_fd),
      session_id_(LaunchSwitch(kAgentSessionSwitch)),
      subagent_id_(LaunchSwitch(kAgentSubagentSwitch)) {
  accept_watch_ = base::FileDescriptorWatcher::WatchReadable(
      listen_fd_,
      base::BindRepeating(&Gateway::OnAcceptable, base::Unretained(this)));
}

Gateway::~Gateway() {
  connection_.reset();
  download_notifiers_.clear();
  accept_watch_.reset();
  close(listen_fd_);
  for (const char* name :
       {"gateway.json", "gateway.json.tmp", "gateway.sock", "gateway.token"}) {
    unlinkat(instance_dir_.get(), name, 0);
  }
  unlinkat(browsers_dir_.get(), instance_id_.c_str(), AT_REMOVEDIR);
}

void Gateway::Emit(std::string_view type,
                   GatewayTab* tab,
                   base::DictValue payload) {
  if (connection_ && connection_->authenticated()) {
    connection_->SendEvent(type, tab, std::move(payload));
  }
}

void Gateway::DropConnection(Connection* connection) {
  if (connection_.get() == connection) {
    connection_.reset();
  }
}

void Gateway::OnAcceptable() {
  int fd = accept4(listen_fd_, nullptr, nullptr, SOCK_NONBLOCK | SOCK_CLOEXEC);
  if (fd < 0) {
    return;
  }
  struct ucred credentials;
  socklen_t length = sizeof(credentials);
  if (getsockopt(fd, SOL_SOCKET, SO_PEERCRED, &credentials, &length) != 0 ||
      credentials.uid != geteuid() || connection_) {
    // Only same-UID peers, and only one control connection per instance.
    close(fd);
    return;
  }
  connection_ = std::make_unique<Connection>(this, fd);
}

void Emit(std::string_view type, GatewayTab* tab, base::DictValue payload) {
  if (g_gateway) {
    g_gateway->Emit(type, tab, std::move(payload));
  }
}

void OnDialogOpened(content::WebContents* contents,
                    content::JavaScriptDialogType type) {
  GatewayTab* tab = GatewayTab::FromWebContents(contents);
  if (!tab) {
    return;
  }
  const char* name = "alert";
  if (type == content::JAVASCRIPT_DIALOG_TYPE_CONFIRM) {
    name = "confirm";
  } else if (type == content::JAVASCRIPT_DIALOG_TYPE_PROMPT) {
    name = "prompt";
  }
  base::DictValue payload;
  payload.Set("dialog_id", tab->OpenDialog());
  payload.Set("dialog_type", name);
  payload.Set("origin",
              contents->GetPrimaryMainFrame()->GetLastCommittedOrigin().Serialize());
  // Dialog text is page-controlled and may contain secrets; never sent.
  payload.Set("message_redacted", true);
  Emit("dialog.opened", tab, std::move(payload));
}

void OnDialogClosed(content::WebContents* contents, bool accepted) {
  GatewayTab* tab = GatewayTab::FromWebContents(contents);
  if (!tab) {
    return;
  }
  std::string dialog_id = tab->TakeDialog();
  if (dialog_id.empty()) {
    return;
  }
  base::DictValue payload;
  payload.Set("dialog_id", dialog_id);
  payload.Set("accepted", accepted);
  Emit("dialog.closed", tab, std::move(payload));
}

bool GatewayEnabled() {
  return base::CommandLine::ForCurrentProcess()->HasSwitch(
      kGatewayInstanceSwitch);
}

// Refuses to start rather than falling back to /tmp or TCP. The runtime
// directory comes from a switch, not the environment, because the browser's
// environment is inherited by native-messaging hosts that need the user's own
// XDG_RUNTIME_DIR. XDG_RUNTIME_DIR is used only when the switch is absent.
std::unique_ptr<Gateway> CreateGateway() {
  const std::string instance_id = LaunchSwitch(kGatewayInstanceSwitch);
  const base::CommandLine* command_line =
      base::CommandLine::ForCurrentProcess();
  std::string runtime;
  if (command_line->HasSwitch(kGatewayRuntimeDirSwitch)) {
    runtime = command_line->GetSwitchValueNative(kGatewayRuntimeDirSwitch);
  } else if (const char* inherited = getenv("XDG_RUNTIME_DIR")) {
    runtime = inherited;
  }
  if (!ValidInstanceId(instance_id) || runtime.empty() || runtime[0] != '/') {
    LOG(ERROR) << "openside gateway: refusing to start without an absolute "
                  "runtime directory and a valid instance id";
    return nullptr;
  }
  const std::string directory = runtime + "/openside/browser/" + instance_id;
  const std::string socket_path = directory + "/gateway.sock";
  struct sockaddr_un address = {};
  if (socket_path.size() >= sizeof(address.sun_path)) {
    LOG(ERROR) << "openside gateway: socket path too long";
    return nullptr;
  }
  base::ScopedFD root = OpenPrivateDirectory(AT_FDCWD, runtime.c_str());
  base::ScopedFD openside_dir =
      root.is_valid() ? EnsurePrivateDirectory(root.get(), "openside")
                      : base::ScopedFD();
  base::ScopedFD browsers_dir =
      openside_dir.is_valid()
          ? EnsurePrivateDirectory(openside_dir.get(), "browser")
          : base::ScopedFD();
  if (!browsers_dir.is_valid()) {
    LOG(ERROR) << "openside gateway: runtime directory is not a private "
                  "(0700, owned by this user) directory";
    return nullptr;
  }
  base::ScopedFD instance;
  if (mkdirat(browsers_dir.get(), instance_id.c_str(), 0700) == 0) {
    instance = OpenPrivateDirectory(browsers_dir.get(), instance_id.c_str());
  }
  if (!instance.is_valid()) {
    LOG(ERROR) << "openside gateway: instance directory already exists";
    return nullptr;
  }
  auto remove_instance = [&] {
    for (const char* name : {"gateway.sock", "gateway.token"}) {
      unlinkat(instance.get(), name, 0);
    }
    unlinkat(browsers_dir.get(), instance_id.c_str(), AT_REMOVEDIR);
  };
  std::array<uint8_t, 32> secret;
  base::RandBytes(secret);
  std::string token = base::HexEncodeLower(secret);
  if (!WriteExclusive(instance.get(), "gateway.token", token + "\n")) {
    remove_instance();
    return nullptr;
  }
  // Bound through the checked descriptor, so the socket lands in the
  // directory verified above even if a path component changed since.
  const std::string bind_path = "/proc/self/fd/" +
                                base::NumberToString(instance.get()) +
                                "/gateway.sock";
  int fd = socket(AF_UNIX, SOCK_STREAM | SOCK_NONBLOCK | SOCK_CLOEXEC, 0);
  address.sun_family = AF_UNIX;
  if (bind_path.size() < sizeof(address.sun_path)) {
    bind_path.copy(address.sun_path, bind_path.size());
  }
  if (fd < 0 || bind_path.size() >= sizeof(address.sun_path) ||
      bind(fd, reinterpret_cast<struct sockaddr*>(&address), sizeof(address)) !=
          0 ||
      fchmodat(instance.get(), "gateway.sock", 0600, 0) != 0 ||
      listen(fd, 4) != 0) {
    if (fd >= 0) {
      close(fd);
    }
    remove_instance();
    return nullptr;
  }
  const int instance_fd = instance.get();
  // From here on the gateway's destructor removes the instance directory.
  auto gateway = std::make_unique<Gateway>(
      instance_id, std::move(browsers_dir), std::move(instance),
      std::move(token), fd);
  // Discovery is published last, so its presence means the socket accepts.
  base::DictValue discovery;
  discovery.Set("protocol", kProtocol);
  base::ListValue versions;
  versions.Append(1);
  discovery.Set("versions", std::move(versions));
  discovery.Set("instance_id", instance_id);
  discovery.Set("pid", static_cast<int>(getpid()));
  discovery.Set("socket", "gateway.sock");
  discovery.Set("token_file", "gateway.token");
  std::string json;
  base::JSONWriter::Write(discovery, &json);
  if (!WriteExclusive(instance_fd, "gateway.json.tmp", json) ||
      renameat(instance_fd, "gateway.json.tmp", instance_fd, "gateway.json") !=
          0) {
    return nullptr;
  }
  return gateway;
}

std::unique_ptr<Gateway>& GatewayHolder() {
  static base::NoDestructor<std::unique_ptr<Gateway>> holder;
  return *holder;
}

}  // namespace

GURL DaemonUiUrl() {
  const GURL ui(LaunchSwitch("openside-daemon-ui-url"));
  if (!ui.is_valid() || !ui.SchemeIs("http") || ui.has_username() ||
      ui.has_password() || ui.has_query() || ui.has_ref() || ui.path() != "/" ||
      (ui.host() != "127.0.0.1" && ui.host() != "[::1]")) {
    return GURL();
  }
  return ui;
}

void StartAgentGateway() {
  if (!GatewayEnabled() || GatewayHolder()) {
    return;
  }
  GatewayHolder() = CreateGateway();
  g_gateway = GatewayHolder().get();
  if (!g_gateway) {
    return;
  }
  javascript_dialogs::OpensideDialogObserver observer;
  observer.opened = &OnDialogOpened;
  observer.closed = &OnDialogClosed;
  javascript_dialogs::SetOpensideDialogObserver(observer);
  for (auto& [sequence, tab] : Registry()) {
    g_gateway->ObserveDownloads(tab->web_contents()->GetBrowserContext());
  }
}

void ShutdownAgentGateway() {
  javascript_dialogs::SetOpensideDialogObserver({});
  g_gateway = nullptr;
  GatewayHolder().reset();
}

void OnTabCreated(content::WebContents* contents) {
  if (!GatewayEnabled() || GatewayTab::FromWebContents(contents)) {
    return;
  }
  GatewayTab::CreateForWebContents(contents);
  if (g_gateway) {
    g_gateway->ObserveDownloads(contents->GetBrowserContext());
  }
  base::SequencedTaskRunner::GetCurrentDefault()->PostTask(
      FROM_HERE,
      base::BindOnce(
          [](base::WeakPtr<content::WebContents> weak) {
            if (weak) {
              if (GatewayTab* tab = GatewayTab::FromWebContents(weak.get())) {
                tab->Announce();
              }
            }
          },
          contents->GetWeakPtr()));
}

void NoteOpenedContents(content::WebContents* opener,
                        content::WebContents* opened) {
  if (opener && opened && opener != opened && IsAgentOriginated(opener) &&
      !IsAgentTab(opened)) {
    AgentOrigin::CreateForWebContents(opened);
  }
}

void OnTabOwnerChanged(content::WebContents* contents) {
  if (GatewayTab* tab = GatewayTab::FromWebContents(contents)) {
    tab->OwnerChanged();
  }
}

void OnDevToolsClientDetached(content::DevToolsAgentHost* host) {
  content::WebContents* contents = host ? host->GetWebContents() : nullptr;
  if (GatewayTab* tab = contents ? GatewayTab::FromWebContents(contents)
                                 : nullptr) {
    tab->ReassertViewport();
  }
}

bool MayActivateTab(content::WebContents* contents) {
  const bool protocol = GatewayEnabled() && content::IsProtocolActivation();
  if (!protocol && !IsAgentOriginated(contents)) {
    return true;
  }
  // Agent descendants carry no identity of their own; they are judged in the
  // launch scope of this dedicated browser.
  const TabOwnership* ownership = TabOwnership::FromWebContents(contents);
  const bool owned = ownership && ownership->owner() == TabOwner::kAgent;
  const std::string session =
      owned ? ownership->session_id() : LaunchSwitch(kAgentSessionSwitch);
  const std::string subagent =
      owned ? ownership->subagent_id() : LaunchSwitch(kAgentSubagentSwitch);
  // Grants come only from native UI, which this build does not issue, so the
  // arbiter state is always kUser for agent activation attempts.
  FocusDecision decision =
      DecideFocus(FocusActor::kAgent, session, subagent, FocusContext());
  if (decision == FocusDecision::kAllow) {
    return true;
  }
  base::DictValue payload;
  payload.Set("request_id", base::Value());
  payload.Set("allowed", false);
  payload.Set("reason", FocusReason(decision));
  payload.Set("owner", "user");
  Emit("focus.decided", GatewayTab::FromWebContents(contents),
       std::move(payload));
  return false;
}

bool MayChangeWindowsOverProtocol() {
  return !GatewayEnabled();
}

bool MayActivateBrowserWindow(BrowserWindowInterface* window) {
  TabStripModel* strip = window ? window->GetTabStripModel() : nullptr;
  if (!strip || strip->empty()) {
    return true;
  }
  // A window with any user tab is the user's window: tab-level activation of
  // its agent tabs is already refused by MayActivateTab.
  for (int i = 0; i < strip->count(); ++i) {
    if (!IsAgentOriginated(strip->GetWebContentsAt(i))) {
      return true;
    }
  }
  content::WebContents* active = strip->GetActiveWebContents();
  return MayActivateTab(active ? active : strip->GetWebContentsAt(0));
}

bool MayShowDownloadUi(content::WebContents* source) {
  return !IsAgentOriginated(source);
}

}  // namespace openside
