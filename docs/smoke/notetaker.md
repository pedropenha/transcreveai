# Smoke manual — Notetaker de reunião (F008/F009)

Checklist de validação manual do detector de reuniões e do fluxo do Notetaker
(T-069 — AC-008-04..06, AC-008-08). O que não dá para automatizar (janelas
reais dos apps de reunião, áudio de loopback, consentimento de primeira
utilização) fica registrado aqui.

Formato: marque o que passou; anote o observado em "Resultado" quando divergir
do esperado.

- Build/data: `__________`
- SO: `__________` · Idioma do app: `__________`

## Pré-requisito: consentimento de primeira utilização

O auto-start exige que o consentimento do Notetaker já tenha sido aceito
(`meeting_consent_acknowledged`). Na primeira reunião, o fluxo é manual:

- [ ] Com consentimento ainda não aceito e auto-start ligado: entrar numa
      reunião real → **esperado:** toast de consentimento (não inicia sozinho);
      aceitar no Hub → reunião seguinte já inicia automaticamente.
      Resultado: \***\*\_\_\*\***

## 1. Detecção por app real (AC-008-08)

Para cada app: abrir uma reunião real (ou janela de teste com áudio), observar
o toast `Reunião detectada`, clicar **Iniciar Notetaker** na primeira linha e
**Nunca perguntar** numa segunda ocorrência quando indicado.

| App                       | Toast aparece | Iniciar funciona | "Nunca perguntar" persiste | Resultado |
| ------------------------- | ------------- | ---------------- | -------------------------- | --------- |
| Zoom (desktop)            | [ ]           | [ ]              | [ ]                        | **\_\_**  |
| Teams (desktop instalado) | [ ]           | [ ]              | [ ]                        | **\_\_**  |
| Teams (pacote MSIX/Store) | [ ]           | [ ]              | [ ]                        | **\_\_**  |
| Google Meet no Chrome     | [ ]           | [ ]              | [ ]                        | **\_\_**  |
| Google Meet no Edge       | [ ]           | [ ]              | [ ]                        | **\_\_**  |
| Webex (desktop)           | [ ]           | [ ]              | [ ]                        | **\_\_**  |

- [ ] Debounce: janela de reunião aberta e fechada em < 5 s não dispara toast.
      Resultado: \***\*\_\_\*\***
- [ ] Fim de chamada detectado ~15 s após fechar a janela/liberar o microfone
      (grace de liberação de mic). Resultado: \***\*\_\_\*\***
- [ ] "Nunca perguntar" persiste após reiniciar o app. Resultado: \***\*\_\_\*\***

## 2. Auto-start (AC-008-04)

- [ ] `Auto-start` global ligado nas configurações: entrar em reunião real →
      **esperado:** sem prompt "Iniciar Notetaker"; apenas confirmação
      `Gravando · <App>` (ou nada) e gravação inicia sozinha.
      Resultado: \***\*\_\_\*\***
- [ ] `Sempre iniciar` por app (`always`): mesmo comportamento só para o app
      escolhido. Resultado: \***\*\_\_\*\***
- [ ] Com reunião já ativa, segunda detecção não abre outra gravação (fica só
      no log). Resultado: \***\*\_\_\*\***

## 3. Fluxo completo da reunião

- [ ] Reunião real gravada com microfone + áudio do sistema (loopback)
      simultâneos. Resultado: \***\*\_\_\*\***
- [ ] Pausar → indicador mostra pausa → retomar → gravação continua no mesmo
      arquivo/sessão. Resultado: \***\*\_\_\*\***
- [ ] Ditar por atalho durante a reunião → ditado vai para o app focado e **não**
      entra no transcript da reunião. Resultado: \***\*\_\_\*\***
- [ ] **Parar reunião** pela bandeja durante gravação automática funciona e leva
      ao processamento. Resultado: \***\*\_\_\*\***
- [ ] Resumo gerado com chave BYOK configurada. Resultado: \***\*\_\_\*\***
- [ ] Botão copiar → Markdown com pauta/decisões/ações vai para a área de
      transferência. Resultado: \***\*\_\_\*\***

## 4. Auto-stop ao fim da reunião (AC-008-05/06)

- [ ] `Auto-stop` ligado: encerrar a reunião no app → em segundos aparece o
      toast `A reunião terminou — finalizando a gravação em 15 s…` com botão
      **Continuar gravando**. Resultado: \***\*\_\_\*\***
- [ ] Sem clicar: a gravação para sozinha após ~15 s e segue para
      processamento/resumo. Resultado: \***\*\_\_\*\***
- [ ] Clicando **Continuar gravando**: toast fecha, gravação segue; encerrar
      de novo a janela não reabre o aviso (detecção já encerrada).
      Resultado: \***\*\_\_\*\***
- [ ] `Auto-stop` desligado: encerrar a reunião **não** mostra o toast e a
      gravação continua até parada manual. Resultado: \***\*\_\_\*\***
- [ ] Reunião iniciada manualmente (sem detecção vinculada) nunca é parada por
      auto-stop. Resultado: \***\*\_\_\*\***

## 5. Ditado (regressão dos smokes anteriores)

- [ ] Ditado no Bloco de Notas → texto colado no cursor. Resultado: \***\*\_\_\*\***
- [ ] Ditado no Chrome (campo de texto) → texto colado. Resultado: \***\*\_\_\*\***
- [ ] Ditado no VS Code → texto colado. Resultado: \***\*\_\_\*\***
- [ ] Janela elevada (ex.: Gerenciador/Notepad como admin) → fallback de UIPI
      informa a limitação sem travar. Resultado: \***\*\_\_\*\***

## Registro de divergências

| Item     | Esperado     | Observado      | Severidade | Follow-up |
| -------- | ------------ | -------------- | ---------- | --------- |
| \_\_\_\_ | **\_\_\_\_** | \***\*\_\*\*** | \_\_\_\_   | \_\_\_\_  |
