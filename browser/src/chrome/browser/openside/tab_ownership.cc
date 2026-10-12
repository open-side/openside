// Copyright 2026 The openside authors
// SPDX-License-Identifier: MIT OR Apache-2.0

#include "chrome/browser/openside/tab_ownership.h"

#include <utility>

#include "build/build_config.h"

#if BUILDFLAG(IS_LINUX)
#include "chrome/browser/openside/agent_gateway.h"
#endif

namespace openside {

TabOwnership::TabOwnership(content::WebContents* contents, TabMetadata metadata)
    : content::WebContentsUserData<TabOwnership>(*contents),
      metadata_(std::move(metadata)) {}

TabOwnership::~TabOwnership() = default;

void TabOwnership::ReturnToUser() {
  metadata_.owner = TabOwner::kUser;
  metadata_.background = false;
#if BUILDFLAG(IS_LINUX)
  OnTabOwnerChanged(&GetWebContents());
#endif
}

WEB_CONTENTS_USER_DATA_KEY_IMPL(TabOwnership);

}  // namespace openside
