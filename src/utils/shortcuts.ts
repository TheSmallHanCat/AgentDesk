export function matchesShortcut(event: Pick<KeyboardEvent,'key'|'ctrlKey'|'altKey'|'shiftKey'|'metaKey'>, binding:string):boolean {
  const keys=binding.toLowerCase().split('+').map(k=>k.trim());const key=keys.pop()
  return !!key&&event.key.toLowerCase()===key&&event.ctrlKey===keys.includes('ctrl')&&event.altKey===keys.includes('alt')&&event.shiftKey===keys.includes('shift')&&event.metaKey===keys.includes('meta')
}
