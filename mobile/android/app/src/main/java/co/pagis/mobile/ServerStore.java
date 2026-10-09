package co.pagis.mobile;

import android.content.Context;
import android.content.SharedPreferences;

/**
 * The origin of the server that the app opens, and the setting Answer on
 * the lock screen of this phone, in {@link SharedPreferences}. It holds the
 * origin alone and never a Sign-In Link, because a link holds a secret.
 */
final class ServerStore {

    private static final String PREFERENCES = "pagis_server";
    private static final String ORIGIN = "origin";
    private static final String LOCK_SCREEN_ANSWERS = "lockScreenAnswers";

    private final SharedPreferences preferences;

    ServerStore(Context context) {
        preferences = context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE);
    }

    /**
     * The stored server, or null when the app shows the Connect screen. A
     * stored value that is not a server origin of this build reads as none.
     */
    ServerOrigin server() {
        return ServerOrigin.parse(preferences.getString(ORIGIN, null), BuildConfig.DEBUG);
    }

    void keep(ServerOrigin server) {
        preferences.edit().putString(ORIGIN, server.serverUrl()).apply();
    }

    void forget() {
        preferences.edit().remove(ORIGIN).apply();
    }

    /**
     * Whether a Notification of an Approval shows Approve once and Deny. It
     * is off when the phone stores no value (ADR-0032).
     */
    boolean lockScreenAnswers() {
        return preferences.getBoolean(LOCK_SCREEN_ANSWERS, false);
    }

    void setLockScreenAnswers(boolean on) {
        preferences.edit().putBoolean(LOCK_SCREEN_ANSWERS, on).apply();
    }
}
