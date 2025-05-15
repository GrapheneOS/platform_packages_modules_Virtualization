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

package com.android.microdroid.test.common;

import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/** This class processes the metrics for both device tests and host tests. */
public final class MetricsProcessor {
    private final String mPrefix;

    public static String getMetricPrefix(String debugTag) {
        return "avf_perf"
                + ((debugTag != null && !debugTag.isEmpty()) ? "[" + debugTag + "]" : "")
                + "/";
    }

    public MetricsProcessor(String prefix) {
        mPrefix = prefix;
    }

    /**
     * Computes the min, max, average and standard deviation of the given metrics and saves them in
     * a {@link Map} with the corresponding keys equal to [mPrefix + name +
     * _[min|max|average|stdev]_ + unit].
     */
    public Map<String, Double> computeStats(
            List<? extends Number> metrics, String name, String unit) {
        List<Double> values = new ArrayList<>(metrics.size());
        for (Number metric : metrics) {
            values.add(metric.doubleValue());
        }
        Collections.sort(values);

        double sum = 0;
        double min = Double.POSITIVE_INFINITY;
        double max = Double.NEGATIVE_INFINITY;
        for (Double d : values) {
            sum += d;
            if (min > d) min = d;
            if (max < d) max = d;
        }
        double avg = sum / values.size();
        double sqSum = 0;
        for (Double d : values) {
            sqSum += (d - avg) * (d - avg);
        }
        double stdDev = Math.sqrt(sqSum / (values.size() - 1));
        double median = Double.NaN;
        if (values.size() > 0) {
            int rank = values.size() / 2;
            if (values.size() % 2 == 0) {
                median = (values.get(rank - 1) + values.get(rank)) / 2;
            } else {
                median = values.get(rank);
            }
        }
        Map<String, Double> stats = new HashMap<String, Double>();
        String prefix = mPrefix + name;
        stats.put(prefix + "_min_" + unit, min);
        stats.put(prefix + "_max_" + unit, max);
        stats.put(prefix + "_average_" + unit, avg);
        stats.put(prefix + "_stdev_" + unit, stdDev);
        stats.put(prefix + "_median_" + unit, median);
        return stats;
    }
}
