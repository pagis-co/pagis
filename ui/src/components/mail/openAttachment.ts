import { Directory, Filesystem } from '@capacitor/filesystem'
import { FileViewer } from '@capacitor/file-viewer'

/** Open a mail attachment with the system viewer of the Mobile App. The
 * request carries the session cookie, and the file stays in the cache. */
export async function openAttachment(url: string, name: string): Promise<void> {
  const response = await fetch(url, { credentials: 'include' })
  if (!response.ok) throw new Error('Could not fetch the attachment.')
  const blob = await response.blob()
  const data = await new Promise<string>((resolve, reject) => {
    const reader = new FileReader()
    reader.onerror = () => reject(new Error('Could not read the attachment.'))
    reader.onload = () => resolve((reader.result as string).split(',')[1])
    reader.readAsDataURL(blob)
  })
  const filename = name.replace(/[^a-zA-Z0-9._-]/g, '_') || 'attachment'
  const { uri } = await Filesystem.writeFile({
    path: `mail-${crypto.randomUUID()}-${filename}`,
    data,
    directory: Directory.Cache,
  })
  // The native viewers take an absolute path, not a file URL.
  await FileViewer.openDocumentFromLocalPath({ path: decodeURIComponent(new URL(uri).pathname) })
}
