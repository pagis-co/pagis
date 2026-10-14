import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { LargeTitle, NavBar, SidebarFrame, SettingsNav, type SidebarFrameProps, type SettingsNavProps } from 'pagis-ui/shell'
import 'pagis-ui/base.css'
import 'pagis-ui/tokens.css'
import 'pagis-ui/fonts'

const sidebar: SidebarFrameProps = {
  userName: 'Alex', role: 'administrator', currentPlace: 'settings', onSignOut: () => {},
}
const settings: SettingsNavProps = {
  isAdministrator: true, currentUrl: 'https://team.example.com/billing',
  administration: {
    origin: 'https://pagis.example.net', path_prefix: '/admin', loopback: false,
    sections: ['people'], links: [{ label: 'Billing', url: 'https://team.example.com/billing' }],
  },
}
export const html = renderToStaticMarkup(
  createElement('div', null,
    createElement(SidebarFrame, sidebar), createElement(SettingsNav, settings),
    createElement(NavBar, { back: { label: 'Settings', onBack: () => {} } }), createElement(LargeTitle, { title: 'Billing' })),
)
