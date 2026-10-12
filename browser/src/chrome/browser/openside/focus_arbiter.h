// Copyright 2026 The openside authors
// SPDX-License-Identifier: MIT OR Apache-2.0

#ifndef CHROME_BROWSER_OPENSIDE_FOCUS_ARBITER_H_
#define CHROME_BROWSER_OPENSIDE_FOCUS_ARBITER_H_

#include <string_view>

namespace openside {

enum class FocusActor { kUserGesture, kAgent };
enum class FocusState { kUser, kAgentGranted };
enum class FocusDecision {
  kAllow,
  kDenyInvalidIdentity,
  kDenyNoUserGrant,
  kDenyDifferentOwner,
};

// Views borrow browser-owned identities only for the synchronous decision.
struct FocusContext {
  FocusState state = FocusState::kUser;
  std::string_view grant_session_id;
  std::string_view grant_subagent_id;
};

FocusDecision DecideFocus(FocusActor actor,
                          std::string_view session_id,
                          std::string_view subagent_id,
                          const FocusContext& context);

}  // namespace openside

#endif  // CHROME_BROWSER_OPENSIDE_FOCUS_ARBITER_H_
