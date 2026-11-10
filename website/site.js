// aWeave website: builds from the latest GitHub release (the visitor's first), the copy button, the nav border
// and scroll reveals. No scroll listeners: everything that reacts to scrolling uses IntersectionObserver.

document.documentElement.classList.add('js')

const REPO = 'DanielHromadka/aweave'
const LATEST = `https://github.com/${REPO}/releases/latest/download/`

/** Every build release.yml publishes, under the stable names it uses. */
const BUILDS = [
  { file: 'aweave-aarch64-apple-darwin.tar.gz', os: 'mac', arch: 'arm64', name: 'macOS', detail: 'Apple silicon', icon: 'ph-apple-logo' },
  { file: 'aweave-x86_64-apple-darwin.tar.gz', os: 'mac', arch: 'x64', name: 'macOS', detail: 'Intel', icon: 'ph-apple-logo' },
  { file: 'aweave-x86_64-unknown-linux-gnu.tar.gz', os: 'linux', arch: 'x64', name: 'Linux', detail: 'x64', icon: 'ph-linux-logo' },
  { file: 'aweave-aarch64-unknown-linux-gnu.tar.gz', os: 'linux', arch: 'arm64', name: 'Linux', detail: 'ARM64', icon: 'ph-linux-logo' },
  { file: 'aweave-x86_64-pc-windows-msvc.zip', os: 'win', arch: 'x64', name: 'Windows', detail: 'x64, experimental', icon: 'ph-windows-logo' }
]

function detectOs() {
  const ua = navigator.userAgent
  const platform = navigator.userAgentData?.platform || navigator.platform || ''
  if (/iPhone|iPad|iPod|Android/i.test(ua)) return null
  if (/Mac/i.test(platform) || /Mac OS X/.test(ua)) return 'mac'
  if (/Win/i.test(platform) || /Windows/.test(ua)) return 'win'
  if (/Linux|X11/i.test(platform)) return 'linux'
  return null
}

async function detectArch() {
  try {
    const hints = await navigator.userAgentData?.getHighEntropyValues?.(['architecture'])
    if (hints?.architecture) return hints.architecture === 'arm' ? 'arm64' : 'x64'
  } catch {
    /* hints unavailable */
  }
  // Safari and Firefox do not say; most Macs sold today are Apple silicon.
  return detectOs() === 'mac' ? 'arm64' : 'x64'
}

function formatSize(bytes) {
  return bytes > 0 ? `${(bytes / 1024 / 1024).toFixed(1)} MB` : ''
}

function renderBuilds(assets, os, arch) {
  const list = document.getElementById('build-list')
  if (!list) return
  const sorted = [...BUILDS].sort((a, b) => (b.os === os && b.arch === arch) - (a.os === os && a.arch === arch))
  list.replaceChildren(
    ...sorted.map((b) => {
      const asset = assets.find((a) => a.name === b.file)
      const li = document.createElement('li')
      const a = document.createElement('a')
      a.href = asset?.browser_download_url || LATEST + b.file
      if (b.os === os && b.arch === arch) a.classList.add('mine')
      const icon = document.createElement('i')
      icon.className = `ph ${b.icon}`
      icon.setAttribute('aria-hidden', 'true')
      const label = document.createElement('span')
      const title = document.createElement('b')
      title.textContent = `${b.name} · ${b.detail}`
      title.translate = false
      const file = document.createElement('small')
      file.textContent = b.file
      label.append(title, file)
      const size = document.createElement('span')
      size.className = 'size'
      size.textContent = formatSize(asset?.size)
      a.append(icon, label, size)
      li.append(a)
      return li
    })
  )
}

async function loadRelease() {
  const os = detectOs()
  const arch = await detectArch()
  renderBuilds([], os, arch)
  try {
    const res = await fetch(`https://api.github.com/repos/${REPO}/releases/latest`, { headers: { Accept: 'application/vnd.github+json' } })
    if (!res.ok) return
    const release = await res.json()
    const version = document.getElementById('release-version')
    if (version && release.tag_name) version.textContent = release.tag_name
    renderBuilds(release.assets || [], os, arch)
  } catch {
    /* offline or rate-limited: the stable /latest/download/ links already work */
  }
}

function setupCopy() {
  for (const button of document.querySelectorAll('[data-copy]')) {
    button.addEventListener('click', async () => {
      const text = document.getElementById(button.dataset.copy)?.textContent || ''
      const label = button.querySelector('span')
      try {
        await navigator.clipboard.writeText(text)
        if (label) label.textContent = 'Copied'
      } catch {
        if (label) label.textContent = 'Select and copy'
      }
      setTimeout(() => label && (label.textContent = 'Copy'), 1800)
    })
  }
}

function setupNav() {
  const nav = document.querySelector('.nav')
  const hero = document.querySelector('.hero')
  if (!nav || !hero || !('IntersectionObserver' in window)) return
  new IntersectionObserver(([entry]) => nav.classList.toggle('scrolled', entry.intersectionRatio < 0.85), {
    threshold: [0.85]
  }).observe(hero)
}

function setupReveal() {
  const items = document.querySelectorAll('.reveal')
  if (!('IntersectionObserver' in window)) {
    items.forEach((el) => el.classList.add('shown'))
    return
  }
  const io = new IntersectionObserver(
    (entries) => {
      for (const e of entries) {
        if (e.isIntersecting) {
          e.target.classList.add('shown')
          io.unobserve(e.target)
        }
      }
    },
    { rootMargin: '0px 0px -10% 0px' }
  )
  items.forEach((el) => io.observe(el))
}

setupCopy()
setupNav()
setupReveal()
loadRelease()
