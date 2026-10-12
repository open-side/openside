// Copyright 2026 The openside authors
// SPDX-License-Identifier: MIT OR Apache-2.0

#include "content/public/browser/openside_protocol_activation.h"

namespace content {

namespace {
bool g_protocol_activation = false;
}  // namespace

ScopedProtocolActivation::ScopedProtocolActivation()
    : previous_(g_protocol_activation) {
  g_protocol_activation = true;
}

ScopedProtocolActivation::~ScopedProtocolActivation() {
  g_protocol_activation = previous_;
}

bool IsProtocolActivation() {
  return g_protocol_activation;
}

}  // namespace content
