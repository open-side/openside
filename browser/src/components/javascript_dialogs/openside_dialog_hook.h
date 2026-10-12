// Copyright 2026 The openside authors
// SPDX-License-Identifier: MIT OR Apache-2.0

#ifndef COMPONENTS_JAVASCRIPT_DIALOGS_OPENSIDE_DIALOG_HOOK_H_
#define COMPONENTS_JAVASCRIPT_DIALOGS_OPENSIDE_DIALOG_HOOK_H_

#include "content/public/common/javascript_dialog_type.h"

namespace content {
class WebContents;
}  // namespace content

namespace javascript_dialogs {

// Observation only: components cannot depend on //chrome, so the openside
// gateway registers plain function pointers here. Observers cannot change
// dialog behavior. UI thread only.
struct OpensideDialogObserver {
  void (*opened)(content::WebContents* contents,
                 content::JavaScriptDialogType type) = nullptr;
  void (*closed)(content::WebContents* contents, bool accepted) = nullptr;
};

void SetOpensideDialogObserver(OpensideDialogObserver observer);
void NotifyOpensideDialogOpened(content::WebContents* contents,
                                content::JavaScriptDialogType type);
void NotifyOpensideDialogClosed(content::WebContents* contents, bool accepted);

}  // namespace javascript_dialogs

#endif  // COMPONENTS_JAVASCRIPT_DIALOGS_OPENSIDE_DIALOG_HOOK_H_
