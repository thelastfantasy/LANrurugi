import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'

import { useShinobuAction, useShinobuStatus } from '../../api/hooks'
import { CollapsibleSection } from '../../components/CollapsibleSection'
import { routes } from '../../routes'
import { FONT_SIZE_10PT } from '../../theme'
import { ActionRow } from './shared'

export function WorkersSection({ onStatus }: { onStatus: (status: string) => void }) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const shinobuStatus = useShinobuStatus()
  const shinobuAction = useShinobuAction()

  return (
    <CollapsibleSection icon="fa-satellite" title={t('Background Workers')}>
      <table style={{ margin: 'auto', fontSize: FONT_SIZE_10PT }}>
        <tbody>
          <tr>
            <td className="option-td">
              <h2 className="ih">{t('Shinobu Status')}</h2>
            </td>
            <td className="config-td">
              {shinobuStatus.data?.is_alive ? (
                <span>
                  {t('The Shinobu File Watcher is currently')}{' '}
                  <h2 className="ih" style={{ display: 'inline', color: 'rgb(26, 165, 26)' }}>
                    👍 {t('OK!')}
                  </h2>
                </span>
              ) : (
                <span>
                  {t('The Shinobu File Watcher is currently')}{' '}
                  <h2 className="ih" style={{ display: 'inline', color: 'rgb(207, 37, 37)' }}>
                    👹 {t('Kaput!')}
                  </h2>
                </span>
              )}{' '}
              ({t('PID:')} <span id="pid">{shinobuStatus.data?.pid}</span>)
              <br />
              {t('This File Watcher is responsible for monitoring your content directory and automatically handling new archives as they come.')}
              <br />
            </td>
          </tr>
          <ActionRow
            id="restart-button"
            label={t('Restart File Watcher')}
            onClick={async () => {
              await shinobuAction.mutateAsync('restart')
              onStatus(t('File Watcher restarted.') ?? '')
            }}
          >
            {t('If Shinobu is dead or unresponsive, you can reboot her by clicking this button.')}
          </ActionRow>
          <ActionRow
            id="open-minion"
            label={t('Open Minion Console')}
            onClick={() => navigate(routes.jobs())}
          >
            {t('The Minion Worker handles spare tasks that are too long to execute within the request/response lifecycle of web applications.')}
            <br />
            {t('The console shows currently running and concluded tasks.')}
          </ActionRow>
        </tbody>
      </table>
    </CollapsibleSection>
  )
}
