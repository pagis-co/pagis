package co.pagis.mobile;

import android.content.Context;
import android.util.Log;
import androidx.annotation.NonNull;
import androidx.work.Data;
import androidx.work.ForegroundInfo;
import androidx.work.Worker;
import androidx.work.WorkerParameters;
import java.io.IOException;
import java.util.function.Supplier;

/**
 * Posts the answer of Approve once or Deny on a Notification of an
 * Approval, with the copy of the Session (ADR-0032). When the daemon takes
 * it, the Notification goes away. Else the failure takes its place. The
 * app keeps no record of the answer. The daemon records the decision
 * (ADR-0004).
 */
public class ApprovalWorker extends Worker {

    /** The attempts of an answer that gets no answer from the daemon, with the backoff of WorkManager between them. */
    static final int MAX_ATTEMPTS = 3;

    private static final String TAG = "Pagis";

    private final ApprovalAnswer answer;
    private final Supplier<SessionCopy> copies;
    private final PushNotifier notifier;

    /** The constructor that WorkManager calls. */
    public ApprovalWorker(@NonNull Context context, @NonNull WorkerParameters parameters) {
        this(context, parameters, new ApprovalAnswer(), () -> SessionCopy.open(context));
    }

    /** @param copies Opens the copy of the Session, which can fail when the app cannot open its key. */
    ApprovalWorker(Context context, WorkerParameters parameters, ApprovalAnswer answer, Supplier<SessionCopy> copies) {
        super(context, parameters);
        this.answer = answer;
        this.copies = copies;
        this.notifier = new PushNotifier(context);
    }

    @NonNull
    @Override
    public Result doWork() {
        Data input = getInputData();
        ApprovalDecision decision = ApprovalDecision.ofAction(input.getString(ApprovalReceiver.EXTRA_ACTION));
        String requestId = input.getString(ApprovalReceiver.EXTRA_REQUEST);
        ServerOrigin server = new ServerStore(getApplicationContext()).server();
        if (decision == null || requestId == null || server == null) {
            Log.e(TAG, "The answer names no decision, no Request or no server.");
            return failed(input);
        }
        SessionCopy copy;
        Session session;
        try {
            copy = copies.get();
            session = copy.read(server);
        } catch (IllegalStateException ex) {
            Log.e(TAG, "The app cannot read the copy of the Session: " + ex.getMessage());
            return failed(input);
        }
        if (session == null) {
            Log.e(TAG, "The app holds no copy of the Session.");
            return failed(input);
        }
        int status;
        try {
            status = answer.post(decision, requestId, server, session);
        } catch (IOException ex) {
            Log.w(TAG, "The decision on a Request did not reach the daemon: " + ex.getMessage());
            return getRunAttemptCount() + 1 < MAX_ATTEMPTS ? Result.retry() : failed(input);
        }
        if (status >= 200 && status < 300) {
            notifier.cancel(input.getString(ApprovalReceiver.EXTRA_ITEM));
            return Result.success();
        }
        Log.e(TAG, "The daemon answered " + status + " to the decision on a Request.");
        if (status == 401) {
            // The Session ended.
            try {
                copy.delete();
            } catch (IllegalStateException ex) {
                Log.e(TAG, ex.getMessage());
            }
        }
        return failed(input);
    }

    /**
     * Before Android 12, WorkManager runs expedited work in a foreground
     * service, which needs a Notification.
     */
    @NonNull
    @Override
    public ForegroundInfo getForegroundInfo() {
        return new ForegroundInfo(PushNotifier.SENDING_ID, notifier.sending());
    }

    /** Show the failure in place of the Notification of the Approval. */
    private Result failed(Data input) {
        String item = input.getString(ApprovalReceiver.EXTRA_ITEM);
        if (item != null) {
            notifier.showAnswerFailed(
                item,
                input.getString(ApprovalReceiver.EXTRA_KIND),
                input.getString(ApprovalReceiver.EXTRA_TITLE),
                input.getString(PushNotifier.EXTRA_NAVIGATE)
            );
        }
        return Result.failure();
    }
}
