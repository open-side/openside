// Copyright 2026 The openside authors
// SPDX-License-Identifier: MIT OR Apache-2.0

#ifndef CHROME_BROWSER_OPENSIDE_TAB_OWNERSHIP_H_
#define CHROME_BROWSER_OPENSIDE_TAB_OWNERSHIP_H_

#include <string>
#include <utility>

#include "content/public/browser/web_contents_user_data.h"

namespace openside {

enum class TabOwner { kAgent, kUser, kExternal };
enum class OpenedBy { kAgent, kUser, kExternal };

struct TabMetadata {
  TabOwner owner = TabOwner::kExternal;
  std::string session_id;
  std::string subagent_id;
  std::string label;
  bool background = false;
  OpenedBy opened_by = OpenedBy::kExternal;
};

class TabOwnership : public content::WebContentsUserData<TabOwnership> {
 public:
  ~TabOwnership() override;

  TabOwner owner() const { return metadata_.owner; }
  const std::string& session_id() const { return metadata_.session_id; }
  const std::string& subagent_id() const { return metadata_.subagent_id; }
  const std::string& label() const { return metadata_.label; }
  bool background() const { return metadata_.background; }
  OpenedBy opened_by() const { return metadata_.opened_by; }
  // Display only; a label never changes ownership or authority.
  void set_label(std::string label) { metadata_.label = std::move(label); }

  // Keep creation provenance when the user takes control of an agent tab.
  void ReturnToUser();

 private:
  friend class content::WebContentsUserData<TabOwnership>;
  TabOwnership(content::WebContents* contents, TabMetadata metadata);

  TabMetadata metadata_;
  WEB_CONTENTS_USER_DATA_KEY_DECL();
};

}  // namespace openside

#endif  // CHROME_BROWSER_OPENSIDE_TAB_OWNERSHIP_H_
