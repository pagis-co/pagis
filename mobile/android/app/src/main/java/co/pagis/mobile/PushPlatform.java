package co.pagis.mobile;

/**
 * What the phone gives for push: the permission of the Person and the FCM
 * token. Each call can wait, so it runs on a worker thread.
 */
interface PushPlatform {

    /** Ask the Person to allow notifications. True when they are allowed. */
    boolean askPermission() throws PushException;

    /** The FCM token of this installation. */
    String token() throws PushException;
}
