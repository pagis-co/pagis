package co.pagis.mobile;

/** Why the app has no Push Subscription. The Product App shows the message to the Person. */
final class PushException extends Exception {

    private static final long serialVersionUID = 1L;

    PushException(String message) {
        super(message);
    }

    PushException(String message, Throwable cause) {
        super(message, cause);
    }
}
