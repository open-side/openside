// Copyright 2026 The openside authors
// SPDX-License-Identifier: MIT OR Apache-2.0

#include "components/javascript_dialogs/openside_dialog_hook.h"

namespace javascript_dialogs {

namespace {

OpensideDialogObserver g_observer;

}  // namespace

void SetOpensideDialogObserver(OpensideDialogObserver observer) {
  g_observer = observer;
}

void NotifyOpensideDialogOpened(content::WebContents* contents,
                                content::JavaScriptDialogType type) {
  if (g_observer.opened) {
    g_observer.opened(contents, type);
  }
}

void NotifyOpensideDialogClosed(content::WebContents* contents, bool accepted) {
  if (g_observer.closed) {
    g_observer.closed(contents, accepted);
  }
}

}  // namespace javascript_dialogs
