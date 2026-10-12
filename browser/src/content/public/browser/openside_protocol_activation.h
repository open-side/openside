// Copyright 2026 The openside authors
// SPDX-License-Identifier: MIT OR Apache-2.0

#ifndef CONTENT_PUBLIC_BROWSER_OPENSIDE_PROTOCOL_ACTIVATION_H_
#define CONTENT_PUBLIC_BROWSER_OPENSIDE_PROTOCOL_ACTIVATION_H_

#include "content/common/content_export.h"

namespace content {

// openside: WebContentsDelegate::ActivateContents cannot tell a page's own
// window.focus() from a DevTools client's Target.activateTarget or
// Page.bringToFront. The DevTools handlers mark their activation with this
// scope so the embedder can judge the requesting actor, not only the target.
// UI thread only.
class CONTENT_EXPORT ScopedProtocolActivation {
 public:
  ScopedProtocolActivation();
  ScopedProtocolActivation(const ScopedProtocolActivation&) = delete;
  ScopedProtocolActivation& operator=(const ScopedProtocolActivation&) = delete;
  ~ScopedProtocolActivation();

 private:
  const bool previous_;
};

// True while a DevTools protocol command is activating a target.
CONTENT_EXPORT bool IsProtocolActivation();

}  // namespace content

#endif  // CONTENT_PUBLIC_BROWSER_OPENSIDE_PROTOCOL_ACTIVATION_H_
