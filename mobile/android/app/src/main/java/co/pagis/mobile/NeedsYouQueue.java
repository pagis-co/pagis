package co.pagis.mobile;

import java.util.Collections;
import java.util.Set;

/** The Needs-You Queue of the Person, as {@code GET /api/v1/needs-you} answers it (ADR-0030). */
final class NeedsYouQueue {

    /** The ids of the items of the queue. */
    final Set<String> items;
    final int count;

    NeedsYouQueue(Set<String> items, int count) {
        this.items = Collections.unmodifiableSet(items);
        this.count = count;
    }
}
