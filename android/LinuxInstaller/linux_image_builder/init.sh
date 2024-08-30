#!/bin/bash
systemctl daemon-reload
systemctl start ttyd && sudo systemctl enable ttyd
systemctl start vsockip && sudo systemctl enable vsockip
