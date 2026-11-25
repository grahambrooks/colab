/* A ring buffer of log lines: the property suite's C sample. */
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define RING_CAPACITY 64
#define LINE_MAX 128
#define MIN(a, b) ((a) < (b) ? (a) : (b))

typedef enum {
    LEVEL_DEBUG,
    LEVEL_INFO,
    LEVEL_WARN,
    LEVEL_ERROR,
} level_t;

typedef struct {
    level_t level;
    uint32_t sequence;
    char text[LINE_MAX];
} line_t;

typedef struct {
    line_t lines[RING_CAPACITY];
    size_t head;
    size_t count;
    uint32_t next_sequence;
} ring_t;

static const char *level_names[] = {"debug", "info", "warn", "error"};

static const char *level_name(level_t level) {
    if (level < LEVEL_DEBUG || level > LEVEL_ERROR) {
        return "unknown";
    }
    return level_names[level];
}

void ring_init(ring_t *ring) {
    memset(ring, 0, sizeof(*ring));
    ring->next_sequence = 1;
}

bool ring_is_full(const ring_t *ring) {
    return ring->count == RING_CAPACITY;
}

void ring_push(ring_t *ring, level_t level, const char *text) {
    size_t slot = (ring->head + ring->count) % RING_CAPACITY;
    if (ring_is_full(ring)) {
        slot = ring->head;
        ring->head = (ring->head + 1) % RING_CAPACITY;
    } else {
        ring->count++;
    }
    line_t *line = &ring->lines[slot];
    line->level = level;
    line->sequence = ring->next_sequence++;
    strncpy(line->text, text, LINE_MAX - 1);
    line->text[LINE_MAX - 1] = '\0';
}

const line_t *ring_get(const ring_t *ring, size_t index) {
    if (index >= ring->count) {
        return NULL;
    }
    return &ring->lines[(ring->head + index) % RING_CAPACITY];
}

size_t ring_count_at_least(const ring_t *ring, level_t level) {
    size_t n = 0;
    for (size_t i = 0; i < ring->count; i++) {
        const line_t *line = ring_get(ring, i);
        if (line != NULL && line->level >= level) {
            n++;
        }
    }
    return n;
}

int ring_format(const ring_t *ring, char *out, size_t size) {
    size_t used = 0;
    for (size_t i = 0; i < ring->count && used < size; i++) {
        const line_t *line = ring_get(ring, i);
        int wrote = snprintf(out + used, size - used, "%u [%s] %s\n",
                             (unsigned)line->sequence, level_name(line->level), line->text);
        if (wrote < 0) {
            return -1;
        }
        used += MIN((size_t)wrote, size - used);
    }
    return (int)used;
}

static int compare_levels(const void *a, const void *b) {
    const line_t *la = a;
    const line_t *lb = b;
    return (int)lb->level - (int)la->level;
}

line_t *ring_sorted_copy(const ring_t *ring) {
    line_t *copy = malloc(sizeof(line_t) * (ring->count ? ring->count : 1));
    if (copy == NULL) {
        return NULL;
    }
    for (size_t i = 0; i < ring->count; i++) {
        copy[i] = *ring_get(ring, i);
    }
    qsort(copy, ring->count, sizeof(line_t), compare_levels);
    return copy;
}

const char *severity(level_t level) {
    switch (level) {
    case LEVEL_ERROR:
        return "page someone";
    case LEVEL_WARN:
        return "look later";
    default:
        break;
    }
    return "ignore";
}

int main(void) {
    ring_t ring;
    ring_init(&ring);
    for (int i = 0; i < 70; i++) {
        char text[32];
        snprintf(text, sizeof(text), "event %d", i);
        ring_push(&ring, i % 7 == 0 ? LEVEL_WARN : LEVEL_INFO, text);
    }
    ring_push(&ring, LEVEL_ERROR, "disk full");
    char buffer[4096];
    int length = ring_format(&ring, buffer, sizeof(buffer));
    printf("%d bytes, %zu warnings or worse\n", length, ring_count_at_least(&ring, LEVEL_WARN));
    line_t *sorted = ring_sorted_copy(&ring);
    if (sorted != NULL) {
        printf("worst: %s (%s)\n", sorted[0].text, severity(sorted[0].level));
        free(sorted);
    }
    return 0;
}
