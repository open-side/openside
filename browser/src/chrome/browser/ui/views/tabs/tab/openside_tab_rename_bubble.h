// Copyright 2026 The openside authors
// SPDX-License-Identifier: MIT OR Apache-2.0

#ifndef CHROME_BROWSER_UI_VIEWS_TABS_TAB_OPENSIDE_TAB_RENAME_BUBBLE_H_
#define CHROME_BROWSER_UI_VIEWS_TABS_TAB_OPENSIDE_TAB_RENAME_BUBBLE_H_

#include <memory>
#include <string>

#include "base/callback_list.h"
#include "base/memory/raw_ptr.h"
#include "components/tabs/public/tab_interface.h"
#include "ui/views/bubble/bubble_border.h"
#include "ui/views/bubble/bubble_dialog_delegate_view.h"
#include "ui/views/controls/textfield/textfield_controller.h"
#include "ui/views/widget/widget.h"

namespace views {
class Textfield;
class View;
}  // namespace views

// The tab context menu's "Rename tab…" editor: one text field, prefilled with
// the tab's current title and anchored on the tab. Enter or closing the
// bubble any other way (clicking elsewhere) stores an edited text as the tab's
// name through TabUIHelper::SetCustomTitle(), where an empty text clears the
// name; Esc, or closing with the prefilled text untouched, changes nothing.
//
// The bubble owns itself and its widget. BubbleDialogDelegateView's
// constructors and widget-owned delegates are limited to friend lists, so this
// is a plain BubbleDialogDelegate with a client-owned widget, deleted from a
// posted task once the widget reports that it closed.
class OpensideTabRenameBubble : public views::BubbleDialogDelegate,
                                public views::TextfieldController {
 public:
  // `arrow` places the bubble beside a vertical tab and below a horizontal
  // one, so it never covers the tab being renamed.
  static void Show(tabs::TabInterface* tab,
                   views::View* anchor_view,
                   views::BubbleBorder::Arrow arrow);

  OpensideTabRenameBubble(const OpensideTabRenameBubble&) = delete;
  OpensideTabRenameBubble& operator=(const OpensideTabRenameBubble&) = delete;
  ~OpensideTabRenameBubble() override;
  // views::TextfieldController:
  bool HandleKeyEvent(views::Textfield* sender,
                      const ui::KeyEvent& key_event) override;

  // The open bubble, if any, for the gateway's test hook, which drives it
  // like a person would: types into the field and presses keys on it.
  static OpensideTabRenameBubble* GetOpenForTesting();
  views::Textfield* field_for_testing() { return field_; }
  views::Widget* widget_for_testing() { return widget_.get(); }

 private:
  OpensideTabRenameBubble(tabs::TabInterface* tab,
                          views::View* anchor_view,
                          views::BubbleBorder::Arrow arrow);

  void OnClosed(views::Widget::ClosedReason reason);
  // The tab is closing or moving to another window: its tab view, the anchor,
  // goes away, so the editor closes without storing anything.
  void OnTabWillDetach(tabs::TabInterface* tab,
                       tabs::TabInterface::DetachReason reason);

  // A handle, not a pointer: the tab can close while the bubble is open.
  const tabs::TabHandle tab_;
  const std::u16string initial_text_;
  raw_ptr<views::Textfield> field_ = nullptr;
  std::unique_ptr<views::Widget> widget_;
  base::CallbackListSubscription tab_detach_subscription_;
  bool tab_detached_ = false;
};

#endif  // CHROME_BROWSER_UI_VIEWS_TABS_TAB_OPENSIDE_TAB_RENAME_BUBBLE_H_
