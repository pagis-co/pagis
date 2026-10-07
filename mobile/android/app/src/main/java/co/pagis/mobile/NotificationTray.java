package co.pagis.mobile;

import android.app.Notification;
import android.app.NotificationManager;
import android.service.notification.StatusBarNotification;
import java.util.ArrayList;
import java.util.List;

/** The Notifications that the app shows, as the clean-up of stale ones reads them. */
interface NotificationTray {

    /** The Notifications that the app shows now. */
    List<Shown> shown();

    /** Cancel the Notification with this tag and id. */
    void cancel(String tag, int id);

    /** One Notification of the app: its tag, which is the item of the queue, and its id. */
    final class Shown {
        /** The item of the queue, or null for a Notification of no item. */
        final String tag;
        final int id;

        Shown(String tag, int id) {
            this.tag = tag;
            this.id = id;
        }
    }

    /**
     * The Notifications of the notification manager. A group summary that
     * Android makes for the Notifications of one kind is not one of them:
     * Android removes it with the last Notification of its group.
     */
    static NotificationTray of(NotificationManager manager) {
        return new NotificationTray() {
            @Override
            public List<Shown> shown() {
                List<Shown> shown = new ArrayList<>();
                for (StatusBarNotification notification : manager.getActiveNotifications()) {
                    if ((notification.getNotification().flags & Notification.FLAG_GROUP_SUMMARY) != 0) continue;
                    shown.add(new Shown(notification.getTag(), notification.getId()));
                }
                return shown;
            }

            @Override
            public void cancel(String tag, int id) {
                manager.cancel(tag, id);
            }
        };
    }
}
