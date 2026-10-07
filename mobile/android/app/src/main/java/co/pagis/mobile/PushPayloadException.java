package co.pagis.mobile;

/** Why a decrypted push is not a payload that the app reads. */
final class PushPayloadException extends Exception {

    private static final long serialVersionUID = 1L;

    enum Reason {
        /** The message has no {@code "web_push": 8030}. */
        NOT_DECLARATIVE_WEB_PUSH,
        /** {@code notification.data.v} is a version that this build does not read. */
        UNKNOWN_VERSION,
        /** The message is not JSON, or a field is missing or of the wrong type. */
        MALFORMED,
    }

    final Reason reason;

    PushPayloadException(Reason reason, String message) {
        super(message);
        this.reason = reason;
    }
}
