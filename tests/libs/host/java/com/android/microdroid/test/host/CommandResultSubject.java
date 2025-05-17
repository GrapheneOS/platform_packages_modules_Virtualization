/*
 * Copyright (C) 2022 The Android Open Source Project
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
package com.android.microdroid.test.host;

import static com.google.common.truth.Truth.assertAbout;

import com.android.tradefed.util.CommandResult;
import com.android.tradefed.util.CommandStatus;

import com.google.common.truth.FailureMetadata;
import com.google.common.truth.IntegerSubject;
import com.google.common.truth.StringSubject;
import com.google.common.truth.Subject;

/** A <a href="https://github.com/google/truth">Truth</a> subject for {@link CommandResult}. */
public class CommandResultSubject extends Subject {
    private final CommandResult mActual;

    public static Factory<CommandResultSubject, CommandResult> command_results() {
        return CommandResultSubject::new;
    }

    public static CommandResultSubject assertThat(CommandResult actual) {
        return assertAbout(command_results()).that(actual);
    }

    private CommandResultSubject(FailureMetadata metadata, CommandResult actual) {
        super(metadata, actual);
        this.mActual = actual;
    }

    public void isSuccess() {
        check("isSuccess()").that(mActual.getStatus()).isEqualTo(CommandStatus.SUCCESS);
    }

    public void isFailed() {
        check("isFailed()").that(mActual.getStatus()).isEqualTo(CommandStatus.FAILED);
    }

    public void isTimedOut() {
        check("isTimedOut()").that(mActual.getStatus()).isEqualTo(CommandStatus.TIMED_OUT);
    }

    public void isException() {
        check("isException()").that(mActual.getStatus()).isEqualTo(CommandStatus.EXCEPTION);
    }

    public IntegerSubject exitCode() {
        return check("exitCode()").that(mActual.getExitCode());
    }

    public StringSubject stdoutTrimmed() {
        return check("stdout()").that(mActual.getStdout().trim());
    }

    public StringSubject stderrTrimmed() {
        return check("stderr()").that(mActual.getStderr().trim());
    }
}
