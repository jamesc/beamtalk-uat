// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0
import { test as setup } from '@playwright/test';
import { prepareToolchain } from '../../../lib/toolchain.ts';

setup('install the fake toolchain', async () => {
  await prepareToolchain();
});
