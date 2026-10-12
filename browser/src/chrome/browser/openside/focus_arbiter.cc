// Copyright 2026 The openside authors
// SPDX-License-Identifier: MIT OR Apache-2.0

#include "chrome/browser/openside/focus_arbiter.h"

namespace openside {

FocusDecision DecideFocus(FocusActor actor,
                          std::string_view session_id,
                          std::string_view subagent_id,
                          const FocusContext& context) {
  if (actor == FocusActor::kUserGesture) {
    return FocusDecision::kAllow;
  }
  if (session_id.empty()) {
    return FocusDecision::kDenyInvalidIdentity;
  }
  if (context.state != FocusState::kAgentGranted) {
    return FocusDecision::kDenyNoUserGrant;
  }
  if (session_id != context.grant_session_id ||
      subagent_id != context.grant_subagent_id) {
    return FocusDecision::kDenyDifferentOwner;
  }
  return FocusDecision::kAllow;
}

}  // namespace openside
