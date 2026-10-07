package app.pagis.mobile;

import java.util.List;

/**
 * Keeps the Notifications of the app on the Needs-You Queue when the app
 * comes to the foreground (ADR-0032). The daemon sends no push when an item
 * leaves the queue (ADR-0030), so the app cancels the Notifications of the
 * items that left. Android shows the count of the Notifications that stay,
 * so the app sets no badge.
 */
final class StaleNotifications {

    private final NeedsYouClient client;
    private final SessionCopy copy;
    private final NotificationTray tray;

    StaleNotifications(NeedsYouClient client, SessionCopy copy, NotificationTray tray) {
        this.client = client;
        this.copy = copy;
        this.tray = tray;
    }

    /**
     * Read the queue of {@code server}, and cancel each Notification whose
     * tag is not an item of it, also one with no tag. A {@code 401} deletes
     * the copy of the Session. A read that fails cancels nothing, and
     * throws. The read goes over the network, so the caller runs it off the
     * main thread.
     */
    void clean(ServerOrigin server) throws NeedsYouException {
        Session session = copy.read(server);
        if (session == null) return;
        // The list comes before the read. A Notification that arrives
        // during the read can be of an item that is newer than the answer,
        // so it stays.
        List<NotificationTray.Shown> shown = tray.shown();
        NeedsYouQueue queue;
        try {
            queue = client.read(server, session);
        } catch (NeedsYouException ex) {
            if (ex.status == 401) copy.delete();
            throw ex;
        }
        for (NotificationTray.Shown notification : shown) {
            if (notification.tag == null || !queue.items.contains(notification.tag)) {
                tray.cancel(notification.tag, notification.id);
            }
        }
    }
}
