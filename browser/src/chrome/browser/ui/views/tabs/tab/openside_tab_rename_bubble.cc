// Copyright 2026 The openside authors
// SPDX-License-Identifier: MIT OR Apache-2.0

#include "chrome/browser/ui/views/tabs/tab/openside_tab_rename_bubble.h"

#include <memory>

#include "base/functional/bind.h"
#include "base/location.h"
#include "base/task/sequenced_task_runner.h"
#include "chrome/browser/ui/tab_ui_helper.h"
#include "chrome/browser/ui/tabs/openside_tab_rename_strings.h"
#include "ui/base/mojom/dialog_button.mojom.h"
#include "ui/base/mojom/ui_base_types.mojom-shared.h"
#include "ui/events/event.h"
#include "ui/events/keycodes/keyboard_codes.h"
#include "ui/gfx/geometry/insets.h"
#include "ui/views/accessibility/view_accessibility.h"
#include "ui/views/controls/textfield/textfield.h"
#include "ui/views/layout/layout_provider.h"

namespace {

// Wide enough for a typical page title without making the bubble a banner.
constexpr int kFieldWidthInChars = 28;

// The bubble that has not closed yet; at most one is open at a time.
OpensideTabRenameBubble* g_open_bubble = nullptr;

}  // namespace

// static
void OpensideTabRenameBubble::Show(tabs::TabInterface* tab,
                                   views::View* anchor_view,
                                   views::BubbleBorder::Arrow arrow) {
  // Owned by itself until OnClosed() hands it to a posted delete.
  auto* bubble = new OpensideTabRenameBubble(tab, anchor_view, arrow);
  bubble->widget_ = views::BubbleDialogDelegate::CreateBubble(
      bubble, base::BindOnce(&OpensideTabRenameBubble::OnClosed,
                             base::Unretained(bubble)));
  bubble->widget_->Show();
  g_open_bubble = bubble;
}

OpensideTabRenameBubble::OpensideTabRenameBubble(
    tabs::TabInterface* tab,
    views::View* anchor_view,
    views::BubbleBorder::Arrow arrow)
    : BubbleDialogDelegate(anchor_view, arrow),
      tab_(tab->GetHandle()),
      initial_text_(TabUIHelper::From(tab)->GetTitle()) {
  SetModalType(ui::mojom::ModalType::kNone);
  SetButtons(static_cast<int>(ui::mojom::DialogButton::kNone));
  set_margins(gfx::Insets(views::LayoutProvider::Get()->GetDistanceMetric(
      views::DISTANCE_RELATED_CONTROL_VERTICAL)));

  auto field = std::make_unique<views::Textfield>();
  field->SetText(initial_text_);
  field->SetDefaultWidthInChars(kFieldWidthInChars);
  field->GetViewAccessibility().SetName(openside::GetTabRenameString(
      openside::TabRenameString::kFieldAccessibleName));
  field->set_controller(this);
  field->SelectAll(/*reversed=*/false);
  field_ = SetContentsView(std::move(field));
  SetInitiallyFocusedView(field_);
  tab_detach_subscription_ = tab->RegisterWillDetach(base::BindRepeating(
      &OpensideTabRenameBubble::OnTabWillDetach, base::Unretained(this)));
}

void OpensideTabRenameBubble::OnTabWillDetach(
    tabs::TabInterface* tab,
    tabs::TabInterface::DetachReason reason) {
  tab_detached_ = true;
  if (widget_ && !widget_->IsClosed()) {
    widget_->CloseWithReason(views::Widget::ClosedReason::kUnspecified);
  }
}

OpensideTabRenameBubble::~OpensideTabRenameBubble() {
  if (g_open_bubble == this) {
    g_open_bubble = nullptr;
  }
  field_ = nullptr;
  // The widget goes first: it still talks to its delegate while it tears down.
  widget_.reset();
}

// static
OpensideTabRenameBubble* OpensideTabRenameBubble::GetOpenForTesting() {
  return g_open_bubble;
}

bool OpensideTabRenameBubble::HandleKeyEvent(views::Textfield* sender,
                                             const ui::KeyEvent& key_event) {
  if (key_event.type() != ui::EventType::kKeyPressed) {
    return false;
  }
  if (key_event.key_code() == ui::VKEY_ESCAPE) {
    widget_->CloseWithReason(views::Widget::ClosedReason::kEscKeyPressed);
    return true;
  }
  if (key_event.key_code() == ui::VKEY_RETURN) {
    widget_->CloseWithReason(views::Widget::ClosedReason::kUnspecified);
    return true;
  }
  return false;
}

// Every way the bubble closes ends here, losing focus included, which commits
// like Enter.
void OpensideTabRenameBubble::OnClosed(views::Widget::ClosedReason reason) {
  // Leaving the prefilled text untouched is not a rename: committing it would
  // freeze the current page title as the tab's name.
  if (!tab_detached_ &&
      reason != views::Widget::ClosedReason::kEscKeyPressed &&
      field_->GetText() != initial_text_) {
    if (tabs::TabInterface* tab = tab_.Get()) {
      TabUIHelper::From(tab)->SetCustomTitle(field_->GetText());
    }
  }
  if (g_open_bubble == this) {
    g_open_bubble = nullptr;
  }
  // Not deleted here: the close can come from inside this bubble's own key
  // handling or the widget's activation change, which still run afterwards.
  base::SequencedTaskRunner::GetCurrentDefault()->DeleteSoon(FROM_HERE, this);
}
