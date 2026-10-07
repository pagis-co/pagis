package app.pagis.mobile;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import androidx.work.Constraints;
import androidx.work.Data;
import androidx.work.ExistingWorkPolicy;
import androidx.work.NetworkType;
import androidx.work.OneTimeWorkRequest;
import androidx.work.OutOfQuotaPolicy;
import androidx.work.WorkManager;

/**
 * Gets the broadcast of Approve once or Deny on a Notification of an
 * Approval (ADR-0032). A receiver has a few seconds and no network
 * thread, so it gives the answer to WorkManager, and {@link ApprovalWorker}
 * posts it.
 */
public class ApprovalReceiver extends BroadcastReceiver {

    /** The id of the Request that the Notification answers. */
    static final String EXTRA_REQUEST = "request";
    /** The item of the Notification, which is its tag. */
    static final String EXTRA_ITEM = "item";
    /** The kind of the item, which is the group of the Notification. */
    static final String EXTRA_KIND = "kind";
    /** The title of the Notification. */
    static final String EXTRA_TITLE = "title";
    /** The key of the input of the work that holds the name of the action. */
    static final String EXTRA_ACTION = "action";

    @Override
    public void onReceive(Context context, Intent intent) {
        // One answer of a Request at a time: a second touch, also on the
        // other action, does not post again while the first answer waits.
        WorkManager.getInstance(context).enqueueUniqueWork(
            "approval:" + intent.getStringExtra(EXTRA_REQUEST),
            ExistingWorkPolicy.KEEP,
            request(intent)
        );
    }

    /**
     * The work of the broadcast {@code intent}. It is expedited, so it runs
     * at once while the app has quota, and it waits for a network.
     */
    static OneTimeWorkRequest request(Intent intent) {
        return new OneTimeWorkRequest.Builder(ApprovalWorker.class)
            .setInputData(input(intent))
            .setExpedited(OutOfQuotaPolicy.RUN_AS_NON_EXPEDITED_WORK_REQUEST)
            .setConstraints(new Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
            .build();
    }

    /** The input of the work: the action, the Request and the Notification of the broadcast {@code intent}. */
    static Data input(Intent intent) {
        return new Data.Builder()
            .putString(EXTRA_ACTION, intent.getAction())
            .putString(EXTRA_REQUEST, intent.getStringExtra(EXTRA_REQUEST))
            .putString(EXTRA_ITEM, intent.getStringExtra(EXTRA_ITEM))
            .putString(EXTRA_KIND, intent.getStringExtra(EXTRA_KIND))
            .putString(EXTRA_TITLE, intent.getStringExtra(EXTRA_TITLE))
            .putString(PushNotifier.EXTRA_NAVIGATE, intent.getStringExtra(PushNotifier.EXTRA_NAVIGATE))
            .build();
    }
}
