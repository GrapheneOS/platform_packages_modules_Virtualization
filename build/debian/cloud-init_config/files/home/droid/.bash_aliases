# Show title of current running command
trap 'echo -ne "\e]0;\$BASH_COMMAND\007"' DEBUG
