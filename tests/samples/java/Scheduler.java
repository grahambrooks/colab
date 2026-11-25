package com.example.scheduler;

import java.time.Duration;
import java.time.Instant;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.PriorityQueue;
import java.util.function.Consumer;
import java.util.stream.Collectors;

/** A job scheduler: the property suite's Java sample. */
public final class Scheduler implements AutoCloseable {

    /** How urgent a job is. */
    public enum Priority {
        LOW(1), NORMAL(5), HIGH(10);

        private final int weight;

        Priority(int weight) {
            this.weight = weight;
        }

        public int weight() {
            return weight;
        }
    }

    /** One scheduled job. */
    public static final class Job {
        private final String name;
        private final Priority priority;
        private final Instant due;
        private final Runnable task;
        private int attempts;

        public Job(String name, Priority priority, Instant due, Runnable task) {
            this.name = name;
            this.priority = priority;
            this.due = due;
            this.task = task;
        }

        public String name() {
            return name;
        }

        @Override
        public String toString() {
            return String.format("Job[%s, %s, due %s]", name, priority, due);
        }
    }

    private final PriorityQueue<Job> queue =
            new PriorityQueue<>(Comparator.comparing((Job j) -> j.due)
                    .thenComparing(j -> -j.priority.weight()));
    private final Map<String, Integer> failures = new HashMap<>();
    private final List<Consumer<Job>> listeners = new ArrayList<>();
    private volatile boolean closed;

    public synchronized void submit(Job job) {
        if (closed) {
            throw new IllegalStateException("scheduler is closed");
        }
        queue.add(job);
    }

    public void onFinished(Consumer<Job> listener) {
        listeners.add(listener);
    }

    public synchronized Optional<Job> next(Instant now) {
        Job head = queue.peek();
        if (head == null || head.due.isAfter(now)) {
            return Optional.empty();
        }
        return Optional.of(queue.poll());
    }

    public int runDue(Instant now) {
        int ran = 0;
        for (Optional<Job> job = next(now); job.isPresent(); job = next(now)) {
            Job j = job.get();
            try {
                j.task.run();
                listeners.forEach(l -> l.accept(j));
                ran++;
            } catch (RuntimeException e) {
                j.attempts++;
                failures.merge(j.name, 1, Integer::sum);
                if (j.attempts < 3) {
                    submit(new Job(j.name, j.priority, now.plus(Duration.ofSeconds(30)), j.task));
                }
            }
        }
        return ran;
    }

    public Map<Priority, List<String>> pendingByPriority() {
        synchronized (this) {
            return queue.stream()
                    .collect(Collectors.groupingBy(j -> j.priority,
                            Collectors.mapping(Job::name, Collectors.toList())));
        }
    }

    public String describe(Priority priority) {
        switch (priority) {
            case LOW:
                return "whenever";
            case HIGH:
                return "now";
            default:
                return "soon";
        }
    }

    @Deprecated
    public static Scheduler withDefaults() {
        Scheduler scheduler = new Scheduler();
        scheduler.onFinished(job -> System.out.println("done: " + job.name()));
        return scheduler;
    }

    @Override
    public synchronized void close() {
        closed = true;
        queue.clear();
    }

    public static void main(String[] args) throws Exception {
        try (Scheduler scheduler = withDefaults()) {
            Instant now = Instant.now();
            scheduler.submit(new Job("backup", Priority.LOW, now, () -> System.out.println("backing up")));
            scheduler.submit(new Job("report", Priority.HIGH, now.minusSeconds(5), () -> {
                throw new IllegalArgumentException("no data");
            }));
            int ran = scheduler.runDue(now.plusSeconds(1));
            System.out.println(ran + " ran, " + scheduler.failures.size() + " failed");
        }
    }
}
