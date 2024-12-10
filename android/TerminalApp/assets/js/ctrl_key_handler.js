/*
 * Copyright (C) 2024 The Android Open Source Project
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
// keyCode 229 means composing text, so get the last character in
// e.target.value.
// keycode 64(@)-95(_) is mapped to a ctrl code
// keycode 97(A)-122(Z) is converted to a small letter, and mapped to ctrl code
window.term.attachCustomKeyEventHandler((e) => {
  if (window.ctrl) {
    keyCode = e.keyCode;
    if (keyCode === 229) {
      keyCode = e.target.value.charAt(e.target.selectionStart - 1).charCodeAt();
    }
    if (64 <= keyCode && keyCode <= 95) {
      input = String.fromCharCode(keyCode - 64);
    } else if (97 <= keyCode && keyCode <= 122) {
      input = String.fromCharCode(keyCode - 96);
    } else {
      return true;
    }
    if (e.type === 'keyup') {
      window.term.input(input);
      e.target.value = e.target.value.slice(0, -1);
      window.ctrl = false;
    }
    return false;
  } else {
    return true;
  }
});
})();