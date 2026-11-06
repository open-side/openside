// Copyright 2026 The openside authors
// SPDX-License-Identifier: MIT OR Apache-2.0

#ifndef CHROME_BROWSER_UI_TABS_OPENSIDE_TAB_RENAME_STRINGS_H_
#define CHROME_BROWSER_UI_TABS_OPENSIDE_TAB_RENAME_STRINGS_H_

#include <string>

#include "base/i18n/rtl.h"
#include "base/strings/string_util.h"

namespace openside {

enum class TabRenameString {
  kRenameMenuItem,
  kResetMenuItem,
  kFieldAccessibleName,
};

// Adding these to generated_resources.grd would regenerate a header most of
// //chrome includes and force a near-full rebuild of every patched build, so
// the overlay keeps its three strings here. The configured ICU locale is the
// application locale Chromium resolved at startup (l10n_util sets it), so the
// table follows the same UI language as the rest of the tab menu.
inline std::u16string GetTabRenameString(TabRenameString id) {
  const bool korean =
      base::StartsWith(base::i18n::GetConfiguredLocale(), "ko",
                       base::CompareCase::INSENSITIVE_ASCII);
  switch (id) {
    case TabRenameString::kRenameMenuItem:
      return korean ? u"탭 이름 바꾸기\u2026" : u"Rename tab\u2026";
    case TabRenameString::kResetMenuItem:
      return korean ? u"탭 이름 초기화" : u"Reset tab name";
    case TabRenameString::kFieldAccessibleName:
      return korean ? u"탭 이름" : u"Tab name";
  }
}

}  // namespace openside

#endif  // CHROME_BROWSER_UI_TABS_OPENSIDE_TAB_RENAME_STRINGS_H_
