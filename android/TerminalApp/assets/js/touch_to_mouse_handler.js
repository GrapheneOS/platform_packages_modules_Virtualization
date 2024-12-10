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
// TODO(b/375326606): consider contribution on
// upstream(https://github.com/xtermjs/xterm.js/issues/3727)
let convertTouchToMouse = false;
function touchHandler(event) {
  const contextmenuByTouch =
      event.type === 'contextmenu' && event.pointerType === 'touch';
  // Only proceed for long touches (contextmenu) or when converting touch to
  // mouse
  if (!contextmenuByTouch && !convertTouchToMouse) {
    return;
  }

  const touch = event.changedTouches ? event.changedTouches[0] : event;

  let type;
  switch (event.type) {
    case 'contextmenu':
      convertTouchToMouse = true;
      type = 'mousedown';
      break;
    case 'touchmove':
      type = 'mousemove';
      break;
    case 'touchend':
      convertTouchToMouse = false;
      type = 'mouseup';
      break;
    default:
      convertTouchToMouse = false;
      return;
  }

  const simulatedEvent = new MouseEvent(type, {
    bubbles: true,
    cancelable: true,
    view: window,
    detail: 1,
    screenX: touch.screenX,
    screenY: touch.screenY,
    clientX: touch.clientX,
    clientY: touch.clientY,
    button: 0,  // left click
  });

  touch.target.dispatchEvent(simulatedEvent);

  // Prevent default behavior for touch events (except contextmenu)
  if (event.type !== 'contextmenu') {
    event.preventDefault();
    event.stopPropagation();
  }
}
const eventOptions = {
  capture: true,
  passive: false
};
document.addEventListener('touchstart', touchHandler, eventOptions);
document.addEventListener('touchmove', touchHandler, eventOptions);
document.addEventListener('touchend', touchHandler, eventOptions);
document.addEventListener('touchcancel', touchHandler, eventOptions);
document.addEventListener('contextmenu', touchHandler, eventOptions);
})();