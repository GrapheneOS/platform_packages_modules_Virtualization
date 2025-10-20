/*
 * Copyright (C) 2025 The Android Open Source Project
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *      http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

(function() {
  var originalLog = console.log;
  console.log = function() {
    console.log.history = console.log.history || [];
    console.log.history.push(arguments);
    originalLog.apply(console, arguments);
    const message = arguments[0];
    const WEBSOCKET_CLOSED_PREFIX = "[ttyd] websocket connection closed with code: ";
    if (typeof message === 'string') {
      if (message.startsWith(WEBSOCKET_CLOSED_PREFIX + "1000")) {
        // 1000 is the code for "normal closure", which means the user closed the
        // tab intentionally.
        TerminalApp.closeTab();
      } else if (message.startsWith(WEBSOCKET_CLOSED_PREFIX)) {
        // Any other code means the connection was closed unexpectedly. Show an
        // error message to the user.
        TerminalApp.showError();
      }
    }
  };
})();
